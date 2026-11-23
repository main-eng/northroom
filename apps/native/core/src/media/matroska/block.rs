//! A Matroska block's header and lacing (RFC 9559 §10): track number, timestamp relative to
//! the cluster, flags, and one frame or several laced together. Lace sizes are Xiph-style (runs
//! of 255), EBML differences, or all equal.

use super::ebml::{signed_variable_integer, variable_integer};
use crate::media::{MediaError, MediaResult};
use std::ops::Range;

/// `SimpleBlock` flag for a frame that can be decoded on its own.
pub(super) const KEYFRAME: u8 = 0x80;
const LACING: u8 = 0x06;
const XIPH_LACING: u8 = 0x02;
const FIXED_LACING: u8 = 0x04;
const EBML_LACING: u8 = 0x06;

/// A block's parsed header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BlockHeader {
    pub(super) track: u64,
    /// In the segment's timestamp units, relative to the cluster's timestamp.
    pub(super) relative_timestamp: i16,
    pub(super) flags: u8,
    /// Offset of the lacing header, or of the frame when there is no lacing.
    pub(super) header_length: usize,
}

/// Parses a block header from its first bytes (12 are enough).
pub(super) fn block_header(bytes: &[u8]) -> MediaResult<BlockHeader> {
    let (track, length, _) = variable_integer(bytes)
        .ok_or_else(|| MediaError("a block's track number is cut off".into()))?;
    let rest = bytes
        .get(length..length + 3)
        .ok_or_else(|| MediaError("a block's header is cut off".into()))?;
    Ok(BlockHeader {
        track,
        relative_timestamp: i16::from_be_bytes([rest[0], rest[1]]),
        flags: rest[2],
        header_length: length + 3,
    })
}

/// The byte range of each frame in a block.
pub(super) fn frames(block: &[u8], header: &BlockHeader) -> MediaResult<Vec<Range<usize>>> {
    let start = header.header_length;
    if header.flags & LACING == 0 {
        return Ok(std::iter::once(start..block.len()).collect());
    }
    let count = usize::from(*block.get(start).ok_or_else(cut_off)?) + 1;
    let mut at = start + 1;
    let sizes = match header.flags & LACING {
        XIPH_LACING => xiph_lace_sizes(block, &mut at, count)?,
        EBML_LACING => ebml_lace_sizes(block, &mut at, count)?,
        FIXED_LACING => return fixed_lace_ranges(block, at, count),
        _ => unreachable!("the lacing bits take four values"),
    };
    let mut ranges = Vec::with_capacity(count);
    for size in sizes {
        let end = at
            .checked_add(size)
            .filter(|end| *end <= block.len())
            .ok_or_else(cut_off)?;
        ranges.push(at..end);
        at = end;
    }
    // The last frame is the rest of the block.
    ranges.push(at..block.len());
    Ok(ranges)
}

fn cut_off() -> MediaError {
    MediaError("a laced block is cut off".into())
}

/// The sizes of all but the last of `count` Xiph-laced frames (each a run of 255s plus the byte
/// that ends it), read from `at`, which is left at the start of the frames.
fn xiph_lace_sizes(block: &[u8], at: &mut usize, count: usize) -> MediaResult<Vec<usize>> {
    let mut sizes = Vec::with_capacity(count);
    for _ in 1..count {
        let mut size = 0;
        loop {
            let byte = *block.get(*at).ok_or_else(cut_off)?;
            *at += 1;
            size += usize::from(byte);
            if byte != 255 {
                break;
            }
        }
        sizes.push(size);
    }
    Ok(sizes)
}

/// The sizes of all but the last of `count` EBML-laced frames (the first size, then each one's
/// difference from the previous), read from `at`, which is left at the start of the frames.
fn ebml_lace_sizes(block: &[u8], at: &mut usize, count: usize) -> MediaResult<Vec<usize>> {
    let mut sizes = Vec::with_capacity(count);
    let (first, length, _) =
        variable_integer(block.get(*at..).ok_or_else(cut_off)?).ok_or_else(cut_off)?;
    *at += length;
    let mut size = usize::try_from(first).map_err(|_| cut_off())?;
    sizes.push(size);
    for _ in 2..count {
        let (difference, length) =
            signed_variable_integer(block.get(*at..).ok_or_else(cut_off)?).ok_or_else(cut_off)?;
        *at += length;
        size = usize::try_from(size as i64 + difference)
            .map_err(|_| MediaError("a laced frame of negative size".into()))?;
        sizes.push(size);
    }
    Ok(sizes)
}

/// `count` fixed-laced frames from `at`: the rest of the block split equally.
fn fixed_lace_ranges(block: &[u8], at: usize, count: usize) -> MediaResult<Vec<Range<usize>>> {
    let total = block.len().checked_sub(at).ok_or_else(cut_off)?;
    if total % count != 0 {
        return Err(MediaError(format!(
            "{total} bytes do not split into {count} equal frames"
        )));
    }
    Ok((0..count)
        .map(|index| at + index * total / count..at + (index + 1) * total / count)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(block: &[u8]) -> Vec<Vec<u8>> {
        let header = block_header(block).unwrap();
        frames(block, &header)
            .unwrap()
            .into_iter()
            .map(|range| block[range].to_vec())
            .collect()
    }

    #[test]
    fn a_block_header_gives_its_track_time_and_flags() {
        let header = block_header(&[0x81, 0xff, 0xfe, 0x80, 0xaa]).unwrap();
        assert_eq!(
            header,
            BlockHeader {
                track: 1,
                relative_timestamp: -2,
                flags: KEYFRAME,
                header_length: 4
            }
        );
        assert!(block_header(&[0x81, 0x00]).is_err());
    }

    #[test]
    fn an_unlaced_block_is_one_frame() {
        assert_eq!(split(&[0x82, 0, 5, 0x80, 1, 2, 3]), vec![vec![1, 2, 3]]);
    }

    #[test]
    fn xiph_lacing_writes_sizes_as_runs_of_255() {
        // Three frames of 256, 2 and the rest (1).
        let mut block = vec![0x81, 0, 0, XIPH_LACING, 2, 255, 1, 2];
        block.extend(std::iter::repeat_n(7, 256));
        block.extend([8, 8, 9]);
        let frames = split(&block);
        assert_eq!(
            frames.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![256, 2, 1]
        );
        assert_eq!(frames[2], vec![9]);
    }

    #[test]
    fn ebml_lacing_writes_the_first_size_then_differences() {
        // Frames of 3, 5 (+2), 4 (-1) and the rest (2).
        let mut block = vec![0x81, 0, 0, EBML_LACING, 3, 0x83, 0xc1, 0xbe];
        block.extend([1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4]);
        assert_eq!(
            split(&block),
            vec![vec![1, 1, 1], vec![2; 5], vec![3; 4], vec![4, 4]]
        );
    }

    #[test]
    fn fixed_lacing_splits_the_block_evenly() {
        let block = [0x81, 0, 0, FIXED_LACING, 2, 1, 1, 2, 2, 3, 3];
        assert_eq!(split(&block), vec![vec![1, 1], vec![2, 2], vec![3, 3]]);
        let uneven = [0x81, 0, 0, FIXED_LACING, 1, 1, 1, 2];
        assert!(frames(&uneven, &block_header(&uneven).unwrap()).is_err());
    }

    #[test]
    fn a_lace_that_runs_past_its_block_is_an_error() {
        let block = [0x81, 0, 0, XIPH_LACING, 1, 9, 1, 2];
        assert!(frames(&block, &block_header(&block).unwrap()).is_err());
    }
}
