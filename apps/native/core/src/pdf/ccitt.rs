//! CCITT fax images (`CCITTFaxDecode`): Group 3 one-dimensional (`K` 0), Group 3 mixed with
//! two-dimensional rows (`K` above 0), and Group 4 (`K` below 0), with or without line markers
//! and byte alignment. Rows are decoded as their changing elements, the columns where the
//! colour flips, each two-dimensional row against the one above, as pdf.js decodes them. The
//! code tables and bit reading are the `fax` crate's.

use fax::maps::{black, mode, white, Mode};
use fax::BitReader;

/// A fax image's parameters, from its `DecodeParms`.
pub struct Parameters {
    pub k: i64,
    pub columns: usize,
    /// Rows to decode, or 0 to decode until the data ends.
    pub rows: usize,
    pub byte_align: bool,
}

/// The most rows decoded when the image doesn't say, so damaged data can't run for ever.
const ROWS_MAX: usize = 1 << 16;

/// One decoded row: the columns where the colour flips, starting white.
type Changes = Vec<usize>;

/// Decodes the image's rows, each as its changing elements, as far as the data allows.
pub fn decode(data: &[u8], parameters: &Parameters) -> Vec<Changes> {
    // The code tables look up to 13 bits ahead, so the last codes need zeros after them; zeros
    // decode to no row.
    let padded: Vec<u8> = data.iter().copied().chain([0; 4]).collect();
    let mut reader = fax::slice_reader(&padded);
    let mut rows: Vec<Changes> = Vec::new();
    let mut above: Changes = Vec::new();
    let wanted = if parameters.rows == 0 {
        ROWS_MAX
    } else {
        parameters.rows
    };
    while rows.len() < wanted {
        let Some(row) = next_row(&mut reader, parameters, &above) else {
            break;
        };
        above = row.clone();
        rows.push(row);
    }
    rows
}

/// Reads one row: past any line marker, then one- or two-dimensional as the scheme and the
/// row's tag bit say. `None` at the end of the data or where it is damaged.
fn next_row(
    reader: &mut impl BitReader,
    parameters: &Parameters,
    above: &[usize],
) -> Option<Changes> {
    if parameters.byte_align && parameters.k < 0 {
        reader.consume(reader.bits_to_byte_boundary()).ok()?;
    }
    if parameters.k >= 0 {
        skip_line_markers(reader, parameters.byte_align)?;
    }
    let two_dimensional = match parameters.k {
        k if k < 0 => true,
        0 => false,
        _ => {
            // A mixed row's tag bit: 1 for one-dimensional.
            let tag = reader.peek(1)?;
            reader.consume(1).ok()?;
            tag == 0
        }
    };
    if two_dimensional {
        two_dimensional_row(reader, parameters.columns, above)
    } else {
        one_dimensional_row(reader, parameters.columns)
    }
}

/// Skips fill bits and line markers (eleven zeros and a one) before a Group 3 row. Six
/// markers in a row end the data.
fn skip_line_markers(reader: &mut impl BitReader, byte_align: bool) -> Option<()> {
    let mut markers = 0;
    loop {
        // Too few bits left for a marker means there is none.
        if byte_align && reader.peek(12) != Some(1) && reader.peek(8) == Some(0) {
            reader.consume(reader.bits_to_byte_boundary()).ok()?;
        }
        match reader.peek(12) {
            Some(1) => {
                reader.consume(12).ok()?;
                markers += 1;
                if markers >= 6 {
                    return None;
                }
            }
            // Fill: zeros before a marker.
            Some(0) => reader.consume(1).ok()?,
            _ => return Some(()),
        }
    }
}

/// A run's length in the colour's code table, its make-up codes summed.
fn run(reader: &mut impl BitReader, black_run: bool) -> Option<usize> {
    let mut total = 0usize;
    loop {
        let length = if black_run {
            black::decode(reader)?
        } else {
            white::decode(reader)?
        };
        total += usize::from(length);
        if length < 64 {
            return Some(total);
        }
    }
}

/// A row of alternating white and black runs.
fn one_dimensional_row(reader: &mut impl BitReader, columns: usize) -> Option<Changes> {
    let mut changes = Vec::new();
    let (mut at, mut black_run) = (0, false);
    while at < columns {
        at += run(reader, black_run)?;
        if at < columns {
            changes.push(at);
        }
        black_run = !black_run;
    }
    Some(changes)
}

/// The row above's first change past `a0` to the colour opposite `black_now`, and the change
/// after it; the row's width where there are none.
fn b1_b2(above: &[usize], a0: Option<usize>, black_now: bool, columns: usize) -> (usize, usize) {
    // Even changes turn white to black, odd ones black to white.
    let wanted_parity = usize::from(black_now);
    let found = above
        .iter()
        .enumerate()
        .position(|(index, &column)| index % 2 == wanted_parity && a0.is_none_or(|a0| column > a0));
    match found {
        Some(index) => (
            above[index],
            above.get(index + 1).copied().unwrap_or(columns),
        ),
        None => (columns, columns),
    }
}

/// A row coded against the row above: passes, horizontal pairs of runs, and vertical offsets.
fn two_dimensional_row(
    reader: &mut impl BitReader,
    columns: usize,
    above: &[usize],
) -> Option<Changes> {
    let mut changes: Changes = Vec::new();
    let mut a0: Option<usize> = None;
    let mut black_now = false;
    while a0.is_none_or(|a0| a0 < columns) {
        let (b1, b2) = b1_b2(above, a0, black_now, columns);
        let start = a0.unwrap_or(0);
        match mode::decode(reader)? {
            Mode::Pass => a0 = Some(b2),
            Mode::Horizontal => {
                let a1 = (start + run(reader, black_now)?).min(columns);
                let a2 = (a1 + run(reader, !black_now)?).min(columns);
                push_change(&mut changes, a1, columns);
                push_change(&mut changes, a2, columns);
                a0 = Some(a2);
            }
            Mode::Vertical(offset) => {
                let a1 =
                    (b1 as i64 + i64::from(offset)).clamp(start as i64, columns as i64) as usize;
                push_change(&mut changes, a1, columns);
                black_now = !black_now;
                a0 = Some(a1);
            }
            Mode::Extension | Mode::EOF => return None,
        }
    }
    Some(changes)
}

/// Adds a change at `column` unless it is the row's end or out of order.
fn push_change(changes: &mut Changes, column: usize, columns: usize) {
    if column < columns && changes.last().is_none_or(|last| column >= *last) {
        changes.push(column);
    }
}

/// Whether each column of a row is black, from its changes.
pub fn blacks(changes: &[usize], columns: usize) -> impl Iterator<Item = bool> + '_ {
    let mut next = 0;
    (0..columns).map(move |column| {
        while next < changes.len() && changes[next] <= column {
            next += 1;
        }
        next % 2 == 1
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows with a black square, as `fax` encodes Group 4.
    fn rows() -> Vec<Vec<bool>> {
        (0..8)
            .map(|row| {
                (0..16)
                    .map(|column| (2..6).contains(&row) && (4..12).contains(&column))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn group_4_rows_decode_against_the_row_above() {
        let mut encoder = fax::encoder::Encoder::new(fax::VecWriter::new());
        for row in rows() {
            let pels = row.into_iter().map(|black| {
                if black {
                    fax::Color::Black
                } else {
                    fax::Color::White
                }
            });
            encoder.encode_line(pels, 16).expect("a line");
        }
        let data = encoder.finish().expect("the end").finish();
        let parameters = Parameters {
            k: -1,
            columns: 16,
            rows: 8,
            byte_align: false,
        };
        let decoded: Vec<Vec<bool>> = decode(&data, &parameters)
            .iter()
            .map(|changes| blacks(changes, 16).collect())
            .collect();
        assert_eq!(decoded, rows());
    }

    /// Writes `bits` (as '0' and '1') into bytes, padded with zeros.
    fn packed(bits: &str) -> Vec<u8> {
        let digits: Vec<u8> = bits
            .bytes()
            .filter(|bit| matches!(bit, b'0' | b'1'))
            .map(|bit| bit - b'0')
            .collect();
        digits
            .chunks(8)
            .map(|chunk| {
                chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |byte, (index, bit)| byte | bit << (7 - index))
            })
            .collect()
    }

    #[test]
    fn group_3_rows_decode_with_and_without_line_markers() {
        // 8 white (10011), 8 black (000101): a row of 16 with its right half black.
        let row = "10011 000101";
        let parameters = Parameters {
            k: 0,
            columns: 16,
            rows: 2,
            byte_align: false,
        };
        let expected = vec![vec![8], vec![8]];
        assert_eq!(
            decode(&packed(&format!("{row} {row}")), &parameters),
            expected
        );
        let marker = "000000000001";
        assert_eq!(
            decode(
                &packed(&format!("{marker} {row} {marker} {row}")),
                &parameters
            ),
            expected
        );
        // Mixed: a tag of 1 before a one-dimensional row, then 0 and a row the same as above
        // (two vertical-0 codes: "1 1").
        let mixed = Parameters {
            k: 2,
            columns: 16,
            rows: 2,
            byte_align: false,
        };
        assert_eq!(
            decode(&packed(&format!("{marker}1 {row} {marker}0 1 1")), &mixed),
            expected
        );
    }
}
