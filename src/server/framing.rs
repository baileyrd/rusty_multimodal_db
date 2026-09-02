//! Length-prefixed binary framing over any `Read`/`Write` (in practice,
//! `std::net::TcpStream`): a 4-byte little-endian length, then that many
//! `bincode`-encoded bytes. See
//! `docs/design/SERVER-QUERY-LAYER-DESIGN.md`'s "Considered options" for
//! why (reuses the existing `bincode` dependency, no new one).

use serde::{de::DeserializeOwned, Serialize};
use std::fmt;
use std::io::{self, Read, Write};

/// Frames larger than this are rejected before any allocation happens —
/// `SERVER-FR-004`'s "a malformed or oversized request is rejected with a
/// typed error response, not a panic or a silently truncated read." 16 MiB
/// is comfortably larger than any request/response this protocol's fixed
/// enum shapes produce in normal use (the largest cases, `RecordList`/
/// `ScanValues`, scale with dataset size, not with anything a length
/// prefix alone inflates) while still bounding what a corrupt or hostile
/// length prefix can make this process allocate.
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    FrameTooLarge { len: u32 },
    Encoding(bincode::Error),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::Io(e) => write!(f, "frame I/O error: {e}"),
            FrameError::FrameTooLarge { len } => {
                write!(
                    f,
                    "frame of {len} bytes exceeds the {MAX_FRAME_BYTES}-byte limit"
                )
            }
            FrameError::Encoding(e) => write!(f, "frame encoding error: {e}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Write one length-prefixed, `bincode`-encoded message.
///
/// # Errors
///
/// Returns [`FrameError::Encoding`] if `msg` fails to serialize (not
/// expected for this protocol's fixed enum shapes, but not assumed
/// infallible either), [`FrameError::FrameTooLarge`] if the encoded
/// payload exceeds [`MAX_FRAME_BYTES`], or [`FrameError::Io`] if the
/// underlying write fails.
pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let payload = crate::codec::encode(msg).map_err(FrameError::Encoding)?;
    let len = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::FrameTooLarge { len });
    }
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&payload)?;
    Ok(())
}

/// Read one length-prefixed, `bincode`-encoded message. The length prefix
/// is checked against [`MAX_FRAME_BYTES`] *before* the payload buffer is
/// allocated, so a corrupt or hostile length prefix can't make this
/// process allocate an unbounded amount of memory before the check runs.
///
/// # Errors
///
/// Returns [`FrameError::FrameTooLarge`] if the length prefix exceeds
/// [`MAX_FRAME_BYTES`], [`FrameError::Io`] if the underlying read fails
/// (including a clean EOF before a full frame arrives), or
/// [`FrameError::Encoding`] if the payload doesn't decode as `T`.
pub fn read_message<R: Read, T: DeserializeOwned>(r: &mut R) -> Result<T, FrameError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf);
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::FrameTooLarge { len });
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    crate::codec::decode(&buf).map_err(FrameError::Encoding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_round_trips_through_a_buffer() {
        let mut buf = Vec::new();
        write_message(&mut buf, &"hello".to_string()).unwrap();
        let mut cursor = &buf[..];
        let decoded: String = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, "hello");
    }

    #[test]
    fn an_oversized_length_prefix_is_rejected_before_reading_the_payload() {
        // A length prefix claiming more than MAX_FRAME_BYTES, with no
        // payload bytes behind it at all — if this allocated first, it
        // would try to read MAX_FRAME_BYTES+1 bytes from an empty buffer
        // and fail with an I/O error instead of FrameTooLarge, proving the
        // check runs before the read.
        let mut buf = Vec::new();
        buf.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_le_bytes());
        let mut cursor = &buf[..];
        let result: Result<String, FrameError> = read_message(&mut cursor);
        assert!(matches!(result, Err(FrameError::FrameTooLarge { .. })));
    }

    /// `BINENC-FR-002`/`STORAGE-018` acceptance criterion 4: a frame whose
    /// length prefix covers a valid `Request` *plus* two junk bytes is an
    /// `Encoding` error, not a silently-accepted request — the codec
    /// rejects trailing bytes. The same frame without the junk decodes.
    #[test]
    fn a_frame_with_bytes_after_the_message_is_an_encoding_error() {
        use crate::server::protocol::Request;
        let payload = crate::codec::encode(&Request::DescribeSchema).unwrap();
        let mut clean = Vec::new();
        clean.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
        clean.extend_from_slice(&payload);
        let mut cursor = &clean[..];
        let decoded: Request = read_message(&mut cursor).unwrap();
        assert!(matches!(decoded, Request::DescribeSchema));

        let mut padded = Vec::new();
        padded.extend_from_slice(&u32::try_from(payload.len() + 2).unwrap().to_le_bytes());
        padded.extend_from_slice(&payload);
        padded.extend_from_slice(&[0xaa, 0xbb]);
        let mut cursor = &padded[..];
        let result: Result<Request, FrameError> = read_message(&mut cursor);
        assert!(
            matches!(result, Err(FrameError::Encoding(_))),
            "expected Encoding, got {result:?}"
        );
    }

    #[test]
    fn a_truncated_frame_is_an_io_error_not_a_panic() {
        let mut buf = Vec::new();
        write_message(&mut buf, &"hello".to_string()).unwrap();
        buf.truncate(buf.len() - 2);
        let mut cursor = &buf[..];
        let result: Result<String, FrameError> = read_message(&mut cursor);
        assert!(matches!(result, Err(FrameError::Io(_))));
    }
}
