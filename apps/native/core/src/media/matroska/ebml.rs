//! EBML, the binary format under Matroska (RFC 8794). Each element is an ID, a size and that
//! many bytes of data. IDs and sizes are variable-length integers whose first byte's leading
//! zeros give their length. A size of all ones means unknown: the element runs until something
//! that can't be inside it.

use crate::media::{MediaError, MediaResult};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};

/// The longest variable-length integer EBML allows.
const LONGEST_VARIABLE_INTEGER: usize = 8;

/// An element header: its ID (with the length marker, as the spec writes IDs), where its data
/// starts, and the data length (`None` if unknown).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Header {
    pub(super) id: u32,
    pub(super) data_start: u64,
    pub(super) size: Option<u64>,
}

impl Header {
    /// Where the element's data ends, if its size is known.
    pub(super) fn end(&self) -> Option<u64> {
        self.size.map(|size| self.data_start + size)
    }
}

/// A variable-length integer from the start of `bytes`: its value without the length marker,
/// its length, and whether every value bit is set (an unknown size).
pub(super) fn variable_integer(bytes: &[u8]) -> Option<(u64, usize, bool)> {
    let first = *bytes.first()?;
    let length = first.leading_zeros() as usize + 1;
    if length > LONGEST_VARIABLE_INTEGER || bytes.len() < length {
        return None;
    }
    let mut value = u64::from(first) & (0xff >> length);
    for byte in &bytes[1..length] {
        value = (value << 8) | u64::from(*byte);
    }
    let all_ones = value == (1_u64 << (7 * length)) - 1;
    Some((value, length, all_ones))
}

/// A signed variable-length integer (EBML lace size differences): the unsigned value minus half
/// its range.
pub(super) fn signed_variable_integer(bytes: &[u8]) -> Option<(i64, usize)> {
    let (value, length, _) = variable_integer(bytes)?;
    let bias = (1_i64 << (7 * length - 1)) - 1;
    Some((value as i64 - bias, length))
}

/// An unsigned integer element's data: big-endian, up to eight bytes, empty for zero.
pub(super) fn unsigned(data: &[u8]) -> MediaResult<u64> {
    if data.len() > 8 {
        return Err(MediaError(format!("an integer of {} bytes", data.len())));
    }
    Ok(data
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte)))
}

/// A signed integer element's data: big-endian two's complement.
pub(super) fn signed(data: &[u8]) -> MediaResult<i64> {
    if data.is_empty() {
        return Ok(0);
    }
    let value = unsigned(data)?;
    let unused = 64 - 8 * data.len() as u32;
    Ok(((value << unused) as i64) >> unused)
}

/// A float element's data: four or eight bytes, empty for zero.
pub(super) fn float(data: &[u8]) -> MediaResult<f64> {
    match data.len() {
        0 => Ok(0.0),
        4 => Ok(f64::from(f32::from_be_bytes([
            data[0], data[1], data[2], data[3],
        ]))),
        8 => Ok(f64::from_be_bytes([
            data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
        ])),
        length => Err(MediaError(format!("a float of {length} bytes"))),
    }
}

/// A string element's data, without the zero bytes that may pad it.
pub(super) fn text(data: &[u8]) -> String {
    let end = data
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(data.len());
    String::from_utf8_lossy(&data[..end]).into_owned()
}

/// The children of a master element held in memory: each one's ID and data.
pub(super) fn children(mut data: &[u8]) -> MediaResult<Vec<(u32, &[u8])>> {
    let mut children = Vec::new();
    while !data.is_empty() {
        let (id, id_length, _) = variable_integer(data)
            .ok_or_else(|| MediaError("an element's ID is cut off".into()))?;
        let id = id_with_marker(id, id_length);
        let (size, size_length, unknown) = variable_integer(&data[id_length..])
            .ok_or_else(|| MediaError("an element's size is cut off".into()))?;
        let start = id_length + size_length;
        let size = if unknown {
            data.len() - start
        } else {
            usize::try_from(size).unwrap_or(usize::MAX)
        };
        let end = start
            .checked_add(size)
            .filter(|end| *end <= data.len())
            .ok_or_else(|| MediaError(format!("element {id:#x} runs past its parent")))?;
        children.push((id, &data[start..end]));
        data = &data[end..];
    }
    Ok(children)
}

/// An ID as the spec writes it, length marker included.
fn id_with_marker(value: u64, length: usize) -> u32 {
    (value | (1 << (7 * length))) as u32
}

/// A file read element by element.
pub(super) struct Source {
    file: BufReader<std::fs::File>,
    position: u64,
    length: u64,
}

impl Source {
    pub(super) fn open(path: &str) -> MediaResult<Self> {
        let file = std::fs::File::open(path)
            .map_err(|error| MediaError(format!("open {path}: {error}")))?;
        let length = file
            .metadata()
            .map_err(|error| MediaError(format!("read {path}: {error}")))?
            .len();
        Ok(Self {
            file: BufReader::with_capacity(64 * 1024, file),
            position: 0,
            length,
        })
    }

    pub(super) fn length(&self) -> u64 {
        self.length
    }

    pub(super) fn seek(&mut self, position: u64) -> MediaResult<()> {
        if position == self.position {
            return Ok(());
        }
        let offset = position as i64 - self.position as i64;
        // Short moves keep the buffer.
        self.file
            .seek_relative(offset)
            .map_err(|error| MediaError(format!("seek in the file: {error}")))?;
        self.position = position;
        Ok(())
    }

    /// The next bytes without consuming them; fewer at end of file.
    fn peek(&mut self, count: usize) -> MediaResult<Vec<u8>> {
        let available = self
            .file
            .fill_buf()
            .map_err(|error| MediaError(format!("read the file: {error}")))?;
        if available.len() >= count {
            return Ok(available[..count].to_vec());
        }
        // The request runs past the buffer: read past it and come back.
        let start = self.position;
        let mut bytes = vec![0; count];
        let read = self.read_up_to(&mut bytes)?;
        bytes.truncate(read);
        self.file
            .seek(SeekFrom::Start(start))
            .map_err(|error| MediaError(format!("seek in the file: {error}")))?;
        self.position = start;
        Ok(bytes)
    }

    fn read_up_to(&mut self, bytes: &mut [u8]) -> MediaResult<usize> {
        let mut read = 0;
        while read < bytes.len() {
            match self.file.read(&mut bytes[read..]) {
                Ok(0) => break,
                Ok(count) => read += count,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(MediaError(format!("read the file: {error}"))),
            }
        }
        self.position += read as u64;
        Ok(read)
    }

    /// Exactly `count` bytes.
    pub(super) fn read(&mut self, count: usize) -> MediaResult<Vec<u8>> {
        if self.position.saturating_add(count as u64) > self.length {
            return Err(MediaError(format!(
                "an element of {count} bytes runs past the end of the file"
            )));
        }
        let mut bytes = vec![0; count];
        if self.read_up_to(&mut bytes)? != count {
            return Err(MediaError("the file ends inside an element".into()));
        }
        Ok(bytes)
    }

    fn read_variable_integer(&mut self) -> MediaResult<Option<(u64, usize, bool)>> {
        let bytes = self.peek(LONGEST_VARIABLE_INTEGER)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let parsed = variable_integer(&bytes).ok_or_else(|| {
            MediaError(format!(
                "a malformed variable-length integer at byte {}",
                self.position
            ))
        })?;
        self.seek(self.position + parsed.1 as u64)?;
        Ok(Some(parsed))
    }

    /// The header of the element at the current position, or `None` at end of file.
    pub(super) fn header(&mut self) -> MediaResult<Option<Header>> {
        if self.position >= self.length {
            return Ok(None);
        }
        let Some((id, id_length, _)) = self.read_variable_integer()? else {
            return Ok(None);
        };
        let Some((size, _, unknown)) = self.read_variable_integer()? else {
            return Err(MediaError(
                "the file ends inside an element's header".into(),
            ));
        };
        Ok(Some(Header {
            id: id_with_marker(id, id_length),
            data_start: self.position,
            size: (!unknown).then_some(size),
        }))
    }

    /// An element's whole data.
    pub(super) fn data(&mut self, header: &Header) -> MediaResult<Vec<u8>> {
        let size = header
            .size
            .ok_or_else(|| MediaError(format!("element {:#x} has no size", header.id)))?;
        self.seek(header.data_start)?;
        self.read(
            usize::try_from(size).map_err(|_| MediaError("an element too large to read".into()))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variable_integers_read_with_their_length_marker_removed() {
        assert_eq!(variable_integer(&[0x81]), Some((1, 1, false)));
        assert_eq!(variable_integer(&[0x40, 0x02]), Some((2, 2, false)));
        assert_eq!(
            variable_integer(&[0x1a, 0x45, 0xdf, 0xa3]),
            Some((0x0a45_dfa3, 4, false))
        );
        // All value bits set means unknown size, at any length.
        assert_eq!(
            variable_integer(&[0xff]).map(|(_, _, unknown)| unknown),
            Some(true)
        );
        assert_eq!(
            variable_integer(&[0x01, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff])
                .map(|(_, length, unknown)| (length, unknown)),
            Some((8, true))
        );
        assert_eq!(variable_integer(&[0x00]), None);
        assert_eq!(variable_integer(&[0x40]), None);
    }

    #[test]
    fn signed_variable_integers_are_offset_by_half_their_range() {
        assert_eq!(signed_variable_integer(&[0xbf]), Some((0, 1)));
        assert_eq!(signed_variable_integer(&[0x80]), Some((-63, 1)));
        assert_eq!(signed_variable_integer(&[0x5f, 0xff]), Some((0, 2)));
        assert_eq!(signed_variable_integer(&[0x60, 0x00]), Some((1, 2)));
    }

    #[test]
    fn numbers_and_strings_read_as_the_specification_writes_them() {
        assert_eq!(unsigned(&[]).ok(), Some(0));
        assert_eq!(unsigned(&[0x0f, 0x42, 0x40]).ok(), Some(1_000_000));
        assert_eq!(signed(&[0xff, 0xfe]).ok(), Some(-2));
        assert_eq!(signed(&[0x7f]).ok(), Some(127));
        assert_eq!(float(&1.5_f32.to_be_bytes()).ok(), Some(1.5));
        assert_eq!(float(&2.25_f64.to_be_bytes()).ok(), Some(2.25));
        assert!(float(&[0, 0, 0]).is_err());
        assert_eq!(text(b"webm\0\0"), "webm");
    }

    #[test]
    fn children_are_split_by_their_sizes() {
        // TrackNumber (0xd7) = 1, CodecID (0x86) = "V_VP9".
        let data = [0xd7, 0x81, 0x01, 0x86, 0x85, b'V', b'_', b'V', b'P', b'9'];
        let children = children(&data).unwrap();
        assert_eq!(children, vec![(0xd7, &[1_u8][..]), (0x86, &b"V_VP9"[..])]);
        assert!(super::children(&[0xd7, 0x85, 0x01]).is_err());
    }
}
