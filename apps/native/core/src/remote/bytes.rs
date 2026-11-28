//! Bytes as the text a message carries them in: standard base64 with padding.

pub use crate::update::sha512::base64 as written;

/// The value of one base64 character.
fn value(character: u8) -> Option<u32> {
    Some(u32::from(match character {
        b'A'..=b'Z' => character - b'A',
        b'a'..=b'z' => character - b'a' + 26,
        b'0'..=b'9' => character - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    }))
}

/// The bytes `text` carries. `None` when it is not base64.
pub fn read(text: &str) -> Option<Vec<u8>> {
    let text = text.trim_end_matches('=').as_bytes();
    let mut bytes = Vec::with_capacity(text.len() * 3 / 4);
    for four in text.chunks(4) {
        // One character alone carries less than a byte.
        if four.len() == 1 {
            return None;
        }
        let mut held = 0;
        for character in four {
            held = (held << 6) | value(*character)?;
        }
        held <<= 6 * (4 - four.len());
        let whole = [(held >> 16) as u8, (held >> 8) as u8, held as u8];
        bytes.extend_from_slice(&whole[..four.len() - 1]);
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_come_back_as_they_were_written_whatever_their_length() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"\x1b[0m\xff\x00\x80"] {
            assert_eq!(read(&written(bytes)).as_deref(), Some(bytes));
        }
        assert_eq!(read("not base64!"), None);
        assert_eq!(read("A"), None);
    }
}
