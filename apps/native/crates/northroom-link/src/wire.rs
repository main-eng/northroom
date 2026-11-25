//! Messages on a link's stream: a length of four bytes, big-endian, then that many bytes.

use crate::MOST_MESSAGE_BYTES;
use iroh::endpoint::{RecvStream, SendStream};

/// The next message, or `None` when the stream ended or sent a message longer than
/// `MOST_MESSAGE_BYTES`.
pub async fn read(stream: &mut RecvStream) -> Option<Vec<u8>> {
    let mut length = [0u8; 4];
    stream.read_exact(&mut length).await.ok()?;
    let length = u32::from_be_bytes(length) as usize;
    if length > MOST_MESSAGE_BYTES {
        return None;
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await.ok()?;
    Some(bytes)
}

/// Sends `bytes` as one message. False when the stream has ended or the message is too long to
/// be read.
pub async fn write(stream: &mut SendStream, bytes: &[u8]) -> bool {
    let Ok(length) = u32::try_from(bytes.len()) else {
        return false;
    };
    if bytes.len() > MOST_MESSAGE_BYTES {
        return false;
    }
    stream.write_all(&length.to_be_bytes()).await.is_ok() && stream.write_all(bytes).await.is_ok()
}
