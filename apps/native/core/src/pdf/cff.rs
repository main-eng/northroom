//! What a bare CFF font program (`FontFile3`, `Type1C` or `CIDFontType0C`) says besides its
//! outlines, which skrifa draws: how many glyphs it has, its built-in encoding, and for a
//! CID-keyed font which glyph each CID is (its charset).

use std::collections::HashMap;

fn be16(data: &[u8], at: usize) -> Option<usize> {
    Some(usize::from(u16::from_be_bytes(
        data.get(at..at + 2)?.try_into().ok()?,
    )))
}

/// An INDEX at `at`: its item count, where it ends, and where each item starts (and the last
/// ends).
fn index(data: &[u8], at: usize) -> Option<(usize, usize, Vec<usize>)> {
    let count = be16(data, at)?;
    if count == 0 {
        return Some((0, at + 2, Vec::new()));
    }
    let size = usize::from(*data.get(at + 2)?);
    let offset = |index: usize| -> Option<usize> {
        let start = at + 3 + index * size;
        Some(
            data.get(start..start + size)?
                .iter()
                .fold(0usize, |value, byte| value << 8 | usize::from(*byte)),
        )
    };
    let offsets: Vec<usize> = (0..=count).map(offset).collect::<Option<_>>()?;
    let base = at + 3 + (count + 1) * size - 1;
    Some((
        count,
        base + offsets[count],
        offsets.iter().map(|offset| base + offset).collect(),
    ))
}

/// Past a real number's nibbles, ended by a nibble of 15.
fn skip_real(dict: &[u8], mut at: usize) -> usize {
    while let Some(&nibbles) = dict.get(at) {
        at += 1;
        if nibbles & 0x0f == 0x0f || nibbles >> 4 == 0x0f {
            break;
        }
    }
    at
}

/// The integer a DICT operand starting with `byte` stands for, and the bytes after `byte` it
/// took; no operand for a byte that starts none.
fn operand(byte: u8, rest: &[u8]) -> Option<(Option<i64>, usize)> {
    let next = || rest.first().copied().map(i64::from);
    Some(match byte {
        28 => (
            Some(i64::from(i16::from_be_bytes(
                rest.get(..2)?.try_into().ok()?,
            ))),
            2,
        ),
        29 => (
            Some(i64::from(i32::from_be_bytes(
                rest.get(..4)?.try_into().ok()?,
            ))),
            4,
        ),
        32..=246 => (Some(i64::from(byte) - 139), 0),
        247..=250 => (Some((i64::from(byte) - 247) * 256 + next()? + 108), 1),
        251..=254 => (Some(-(i64::from(byte) - 251) * 256 - next()? - 108), 1),
        _ => (None, 0),
    })
}

/// Operators of two bytes (12 and another) are known as 1200 and the other.
const ESCAPED: u16 = 1200;
const CHARSET: u16 = 15;
const ENCODING: u16 = 16;
const CHAR_STRINGS: u16 = 17;
/// Registry, ordering and supplement: present only in a CID-keyed font.
const ROS: u16 = ESCAPED + 30;

/// A DICT's operators and their operands; a real number counts as 0.
fn dict(data: &[u8]) -> HashMap<u16, Vec<i64>> {
    let mut entries = HashMap::new();
    let mut operands: Vec<i64> = Vec::new();
    let mut at = 0;
    while let Some(&byte) = data.get(at) {
        at += 1;
        match byte {
            12 => {
                let Some(&second) = data.get(at) else {
                    break;
                };
                at += 1;
                entries.insert(ESCAPED + u16::from(second), std::mem::take(&mut operands));
            }
            0..=21 => {
                entries.insert(u16::from(byte), std::mem::take(&mut operands));
            }
            30 => {
                at = skip_real(data, at);
                operands.push(0);
            }
            _ => {
                let Some((value, used)) = data.get(at..).and_then(|rest| operand(byte, rest))
                else {
                    break;
                };
                operands.extend(value);
                at += used;
            }
        }
    }
    entries
}

/// The program's first (and only) Top DICT.
fn top_dict(cff: &[u8]) -> Option<HashMap<u16, Vec<i64>>> {
    let header = usize::from(*cff.get(2)?);
    let (_, after_names, _) = index(cff, header)?;
    let (_, _, tops) = index(cff, after_names)?;
    Some(dict(cff.get(*tops.first()?..*tops.get(1)?)?))
}

fn offset_of(top: &HashMap<u16, Vec<i64>>, operator: u16) -> Option<usize> {
    top.get(&operator)?
        .last()
        .map(|value| (*value).max(0) as usize)
}

/// How many glyphs the program holds.
pub fn glyph_count(cff: &[u8]) -> Option<usize> {
    let top = top_dict(cff)?;
    Some(index(cff, offset_of(&top, CHAR_STRINGS)?)?.0)
}

/// A program's built-in encoding: the standard one, or its own codes for its glyphs.
pub enum Encoding {
    Standard,
    /// Each code's glyph.
    Glyphs(HashMap<u32, u32>),
}

/// The program's built-in encoding, if it says one (the expert encoding is not read).
pub fn encoding(cff: &[u8]) -> Option<Encoding> {
    let top = top_dict(cff)?;
    if top.contains_key(&ROS) {
        return None;
    }
    let at = match offset_of(&top, ENCODING).unwrap_or(0) {
        0 => return Some(Encoding::Standard),
        1 => return None,
        at => at,
    };
    let format = *cff.get(at)?;
    let (mut glyphs, after) = encoded_glyphs(cff, at, format & 0x7f)?;
    if format & 0x80 != 0 {
        add_supplements(cff, &top, after, &mut glyphs);
    }
    Some(Encoding::Glyphs(glyphs))
}

/// An encoding's codes for its glyphs in order (format 0, a code each; format 1, ranges of
/// codes), and where they end.
fn encoded_glyphs(cff: &[u8], at: usize, format: u8) -> Option<(HashMap<u32, u32>, usize)> {
    let count = usize::from(*cff.get(at + 1)?);
    let codes: Vec<u32> = match format {
        0 => cff
            .get(at + 2..at + 2 + count)?
            .iter()
            .map(|code| u32::from(*code))
            .collect(),
        1 => cff
            .get(at + 2..at + 2 + count * 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|range| u32::from(range[0])..=u32::from(range[0]) + u32::from(range[1]))
            .collect(),
        _ => return None,
    };
    let after = at + 2 + if format == 0 { count } else { count * 2 };
    Some((codes.into_iter().zip(1u32..).collect(), after))
}

/// Each glyph's string ID (or, in a CID-keyed program, its CID), from the charset at `at`.
fn charset(cff: &[u8], at: usize, count: usize) -> Option<Vec<u32>> {
    let mut ids = vec![0u32];
    let format = *cff.get(at)?;
    let mut cursor = at + 1;
    while ids.len() < count {
        let first = be16(cff, cursor)? as u32;
        let (left, used) = match format {
            0 => (0, 2),
            1 => (u32::from(*cff.get(cursor + 2)?), 3),
            2 => (be16(cff, cursor + 2)? as u32, 4),
            _ => return None,
        };
        ids.extend((first..=first + left).take(count - ids.len()));
        cursor += used;
    }
    Some(ids)
}

/// An encoding's supplements, after its codes at `at`: more codes, each naming its glyph by
/// string ID.
fn add_supplements(
    cff: &[u8],
    top: &HashMap<u16, Vec<i64>>,
    at: usize,
    glyphs: &mut HashMap<u32, u32>,
) -> Option<()> {
    let ids = charset(cff, offset_of(top, CHARSET)?, glyph_count(cff)?)?;
    let count = usize::from(*cff.get(at)?);
    for entry in cff.get(at + 1..at + 1 + count * 3)?.as_chunks::<3>().0 {
        let id = u32::from(u16::from_be_bytes([entry[1], entry[2]]));
        if let Some(glyph) = ids.iter().position(|known| *known == id) {
            glyphs.insert(u32::from(entry[0]), glyph as u32);
        }
    }
    Some(())
}

/// A CID-keyed program's glyph for each CID, from its charset; `None` for a font not keyed by
/// CID.
pub fn cid_glyphs(cff: &[u8]) -> Option<HashMap<u32, u32>> {
    let top = top_dict(cff)?;
    top.get(&ROS)?;
    let ids = charset(cff, offset_of(&top, CHARSET)?, glyph_count(cff)?)?;
    Some(
        ids.into_iter()
            .enumerate()
            .map(|(glyph, cid)| (cid, glyph as u32))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_programs_glyphs_and_standard_encoding_are_read() {
        let cff = include_bytes!("../../tests/fixtures/pdf/square.cff");
        assert_eq!(glyph_count(cff), Some(2));
        assert!(matches!(encoding(cff), Some(Encoding::Standard)));
        assert!(cid_glyphs(cff).is_none());
    }
}
