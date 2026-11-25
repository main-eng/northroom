//! Reads an Excel 97–2003 workbook (`.xls`): a compound file whose `Workbook` stream is a run
//! of BIFF8 records. The workbook's globals come first (sheets, shared strings, number formats,
//! cell formats, fonts), then each sheet's cells. Each visible worksheet becomes a sheet of
//! formatted cell text with the file's bold and column widths; formulas show their saved result.
//!
//! Left out: Excel 5 and older (BIFF5 and before), fills, colours, merged cells, charts and
//! pictures, as in `workbook.rs`.

use crate::compound::Compound;
use crate::sheets::spreadsheet::{self, Alignment, Cell, Sheet, Workbook};
use crate::sheets::workbook::{
    built_in_format, format_number, place_at, DateSystem, Result, WorkbookError, COLUMN_LIMIT,
    ROW_LIMIT,
};

/// Record types read.
const BOF: u16 = 0x0809;
const EOF: u16 = 0x000a;
const CONTINUE: u16 = 0x003c;
const SHEET: u16 = 0x0085;
const SHARED_STRINGS: u16 = 0x00fc;
const FORMAT: u16 = 0x041e;
const CELL_FORMAT: u16 = 0x00e0;
const FONT: u16 = 0x0031;
const DATE_MODE: u16 = 0x0022;
const LABEL_SHARED: u16 = 0x00fd;
const LABEL: u16 = 0x0204;
const NUMBER: u16 = 0x0203;
const RK: u16 = 0x027e;
const MULTIPLE_RK: u16 = 0x00bd;
const BOOLEAN_OR_ERROR: u16 = 0x0205;
const FORMULA: u16 = 0x0006;
const FORMULA_STRING: u16 = 0x0207;
const COLUMN_INFO: u16 = 0x007d;
/// BIFF8's version in its BOF record.
const BIFF8: u16 = 0x0600;
/// A font weight at and above which text is bold.
const BOLD_WEIGHT: u16 = 700;
/// Excel's column width unit: a 256th of a character.
const WIDTH_UNITS_PER_CHARACTER: f64 = 256.0;
/// Points per character and padding, as `workbook.rs` sizes Excel's widths.
const COLUMN_WIDTH_CHARACTER: f64 = 8.0;
const COLUMN_WIDTH_PADDING: f64 = 17.0;
/// The format of a cell without one.
const GENERAL: &str = "General";

fn fail<T>(why: &str) -> Result<T> {
    Err(WorkbookError(why.to_owned()))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..at + 2)
        .map_or(0, |two| u16::from_le_bytes([two[0], two[1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes.get(at..at + 4).map_or(0, |four| {
        u32::from_le_bytes(four.try_into().expect("four bytes"))
    })
}

fn f64_at(bytes: &[u8], at: usize) -> f64 {
    bytes.get(at..at + 8).map_or(f64::NAN, |eight| {
        f64::from_le_bytes(eight.try_into().expect("eight bytes"))
    })
}

/// One record: its type and its data, with any continuation records after it kept apart, since
/// a string that runs into one starts again with its own flags.
struct Record<'a> {
    kind: u16,
    parts: Vec<&'a [u8]>,
}

impl Record<'_> {
    fn data(&self) -> &[u8] {
        self.parts[0]
    }
}

/// The records of a stream from `start`, up to the next EOF at the same depth.
fn records(stream: &[u8], start: usize) -> Vec<Record<'_>> {
    let mut found: Vec<Record> = Vec::new();
    let mut at = start;
    while at + 4 <= stream.len() {
        let kind = u16_at(stream, at);
        let length = usize::from(u16_at(stream, at + 2));
        let Some(data) = stream.get(at + 4..at + 4 + length) else {
            break;
        };
        at += 4 + length;
        match (kind, found.last_mut()) {
            (CONTINUE, Some(last)) => last.parts.push(data),
            _ => found.push(Record {
                kind,
                parts: vec![data],
            }),
        }
        if kind == EOF {
            break;
        }
    }
    found
}

/// Reads strings that may cross from one record part into the next.
struct Strings<'a> {
    parts: &'a [&'a [u8]],
    part: usize,
    at: usize,
}

impl Strings<'_> {
    fn bytes(&mut self, count: usize) -> Vec<u8> {
        let mut taken = Vec::with_capacity(count);
        while taken.len() < count {
            let Some(part) = self.parts.get(self.part) else {
                break;
            };
            if self.at >= part.len() {
                self.part += 1;
                self.at = 0;
                continue;
            }
            let end = (self.at + count - taken.len()).min(part.len());
            taken.extend_from_slice(&part[self.at..end]);
            self.at = end;
        }
        taken
    }

    /// `count` characters, single-byte or UTF-16 as `wide` says. A character run that crosses
    /// into the next part starts that part with a flags byte of its own.
    fn characters(&mut self, count: usize, mut wide: bool) -> String {
        let mut units: Vec<u16> = Vec::with_capacity(count);
        while units.len() < count {
            let Some(part) = self.parts.get(self.part) else {
                break;
            };
            if self.at >= part.len() {
                self.part += 1;
                self.at = 0;
                wide = self.bytes(1).first().is_some_and(|flags| flags & 1 == 1);
                continue;
            }
            if wide {
                let two = self.bytes(2);
                units.push(u16::from_le_bytes([two[0], *two.get(1).unwrap_or(&0)]));
            } else {
                units.push(u16::from(self.bytes(1)[0]));
            }
        }
        String::from_utf16_lossy(&units)
    }

    /// An `XLUnicodeRichExtendedString`: a count, flags, then its characters, skipping the
    /// formatting runs and phonetic data after them.
    fn rich(&mut self) -> String {
        let count = {
            let two = self.bytes(2);
            usize::from(u16::from_le_bytes([two[0], *two.get(1).unwrap_or(&0)]))
        };
        let flags = self.bytes(1).first().copied().unwrap_or(0);
        let runs = if flags & 0x08 != 0 {
            usize::from(u16_at(&self.bytes(2), 0))
        } else {
            0
        };
        let extra = if flags & 0x04 != 0 {
            u32_at(&self.bytes(4), 0) as usize
        } else {
            0
        };
        let text = self.characters(count, flags & 1 == 1);
        self.bytes(runs * 4 + extra);
        text
    }
}

/// A short string in a record: a count of one or two bytes, flags, then the characters.
fn short_string(data: &[u8], at: usize, wide_count: bool) -> String {
    let (count, flags_at) = if wide_count {
        (usize::from(u16_at(data, at)), at + 2)
    } else {
        (usize::from(*data.get(at).unwrap_or(&0)), at + 1)
    };
    let parts = [data];
    let mut strings = Strings {
        parts: &parts,
        part: 0,
        at: flags_at + 1,
    };
    let wide = data.get(flags_at).is_some_and(|flags| flags & 1 == 1);
    strings.characters(count, wide)
}

/// A sheet the workbook lists: where its records start and its name.
struct Listed {
    start: usize,
    name: String,
}

/// What the workbook's globals say.
#[derive(Default)]
struct Globals {
    sheets: Vec<Listed>,
    strings: Vec<String>,
    formats: Vec<(u16, String)>,
    /// Each cell format's number format and font.
    cell_formats: Vec<(u16, u16)>,
    bold_fonts: Vec<bool>,
    system: Option<DateSystem>,
}

impl Globals {
    fn read(stream: &[u8]) -> Result<Self> {
        let mut globals = Globals::default();
        let found = records(stream, 0);
        let Some(first) = found.first().filter(|first| first.kind == BOF) else {
            return fail("The workbook stream does not start with a BOF record.");
        };
        if u16_at(first.data(), 0) != BIFF8 {
            return fail("The workbook is older than Excel 97.");
        }
        for record in &found {
            globals.take(record);
        }
        Ok(globals)
    }

    fn take(&mut self, record: &Record) {
        let data = record.data();
        match record.kind {
            // Only visible worksheets.
            SHEET if data.get(4) == Some(&0) && data.get(5) == Some(&0) => {
                self.sheets.push(Listed {
                    start: u32_at(data, 0) as usize,
                    name: short_string(data, 6, false),
                });
            }
            SHARED_STRINGS => {
                let count = u32_at(data, 4) as usize;
                let mut strings = Strings {
                    parts: &record.parts,
                    part: 0,
                    at: 8,
                };
                self.strings = (0..count.min(ROW_LIMIT as usize))
                    .map(|_| strings.rich())
                    .collect();
            }
            FORMAT => self
                .formats
                .push((u16_at(data, 0), short_string(data, 2, true))),
            CELL_FORMAT => self.cell_formats.push((u16_at(data, 2), u16_at(data, 0))),
            FONT => self.bold_fonts.push(u16_at(data, 6) >= BOLD_WEIGHT),
            DATE_MODE if u16_at(data, 0) == 1 => self.system = Some(DateSystem::From1904),
            _ => {}
        }
    }

    /// The number format and boldness of cell format `index`.
    fn look(&self, index: u16) -> (String, bool) {
        let Some(&(format, font)) = self.cell_formats.get(usize::from(index)) else {
            return (GENERAL.to_owned(), false);
        };
        let code = self
            .formats
            .iter()
            .find(|(id, _)| *id == format)
            .map(|(_, code)| code.clone())
            .or_else(|| built_in_format(f64::from(format)).map(str::to_owned))
            .unwrap_or_else(|| GENERAL.to_owned());
        // Font 4 is never written, so indices after it are one more than their place.
        let place = if font >= 4 { font - 1 } else { font };
        let bold = self
            .bold_fonts
            .get(usize::from(place))
            .copied()
            .unwrap_or(false);
        (code, bold)
    }

    fn system(&self) -> DateSystem {
        self.system.unwrap_or(DateSystem::From1900)
    }
}

/// A number stored as an RK value: 30 bits of an integer or the top of a double, maybe
/// hundredths.
fn rk_value(rk: u32) -> f64 {
    let value = if rk & 2 != 0 {
        f64::from((rk as i32) >> 2)
    } else {
        f64::from_bits(u64::from(rk & 0xffff_fffc) << 32)
    };
    if rk & 1 != 0 {
        value / 100.0
    } else {
        value
    }
}

/// A cell's value as read.
enum Value {
    Number(f64),
    Text(String),
    Boolean(bool),
    Error(u8),
}

/// The text an error code shows.
fn error_text(code: u8) -> &'static str {
    match code {
        0x00 => "#NULL!",
        0x07 => "#DIV/0!",
        0x0f => "#VALUE!",
        0x17 => "#REF!",
        0x1d => "#NAME?",
        0x24 => "#NUM!",
        _ => "#N/A",
    }
}

/// A sheet being read: its cells and column widths.
struct Reading<'a> {
    globals: &'a Globals,
    rows: Vec<Vec<Option<Cell>>>,
    columns: usize,
    widths: Vec<(usize, usize, f64)>,
    /// A formula whose text result follows in the next record.
    pending: Option<(usize, usize, u16)>,
}

impl Reading<'_> {
    fn put(&mut self, row: usize, column: usize, format: u16, value: Value) {
        if row >= ROW_LIMIT as usize || column >= COLUMN_LIMIT as usize {
            return;
        }
        let (code, bold) = self.globals.look(format);
        let (text, alignment) = match value {
            Value::Number(number) => (
                format_number(number, &code, self.globals.system())
                    .unwrap_or_else(|_| number.to_string()),
                Alignment::End,
            ),
            Value::Text(text) => (text, Alignment::Start),
            Value::Boolean(on) => (
                (if on { "TRUE" } else { "FALSE" }).to_owned(),
                Alignment::Center,
            ),
            Value::Error(code) => (error_text(code).to_owned(), Alignment::Center),
        };
        if text.is_empty() {
            return;
        }
        if self.rows.len() <= row {
            self.rows.resize(row + 1, Vec::new());
        }
        place_at(
            &mut self.rows[row],
            column,
            Some(Cell {
                text,
                alignment,
                bold,
            }),
        );
        self.columns = self.columns.max(column + 1);
    }

    fn take_formula(&mut self, data: &[u8], place: (usize, usize, u16)) {
        let (row, column, format) = place;
        let result = data.get(6..14).unwrap_or(&[]);
        if result.len() < 8 || result[6..8] != [0xff, 0xff] {
            self.put(row, column, format, Value::Number(f64_at(data, 6)));
            return;
        }
        match result[0] {
            0 => self.pending = Some(place),
            1 => self.put(row, column, format, Value::Boolean(result[2] != 0)),
            2 => self.put(row, column, format, Value::Error(result[2])),
            _ => {}
        }
    }

    fn take_multiple(&mut self, data: &[u8], row: usize) {
        let first = usize::from(u16_at(data, 2));
        let count = data.len().saturating_sub(6) / 6;
        for index in 0..count {
            let at = 4 + index * 6;
            let value = rk_value(u32_at(data, at + 2));
            self.put(row, first + index, u16_at(data, at), Value::Number(value));
        }
    }

    fn take(&mut self, record: &Record) {
        let data = record.data();
        let place = (
            usize::from(u16_at(data, 0)),
            usize::from(u16_at(data, 2)),
            u16_at(data, 4),
        );
        let (row, column, format) = place;
        match record.kind {
            LABEL_SHARED => {
                let text = self.globals.strings.get(u32_at(data, 6) as usize).cloned();
                self.put(row, column, format, Value::Text(text.unwrap_or_default()));
            }
            LABEL => self.put(
                row,
                column,
                format,
                Value::Text(short_string(data, 6, true)),
            ),
            NUMBER => self.put(row, column, format, Value::Number(f64_at(data, 6))),
            RK => self.put(
                row,
                column,
                format,
                Value::Number(rk_value(u32_at(data, 6))),
            ),
            MULTIPLE_RK => self.take_multiple(data, row),
            BOOLEAN_OR_ERROR => {
                let value = data.get(6).copied().unwrap_or(0);
                let shown = if data.get(7) == Some(&1) {
                    Value::Error(value)
                } else {
                    Value::Boolean(value != 0)
                };
                self.put(row, column, format, shown);
            }
            FORMULA => self.take_formula(data, place),
            FORMULA_STRING => {
                if let Some((row, column, format)) = self.pending.take() {
                    self.put(
                        row,
                        column,
                        format,
                        Value::Text(short_string(data, 0, true)),
                    );
                }
            }
            COLUMN_INFO => self.widths.push((
                usize::from(u16_at(data, 0)),
                usize::from(u16_at(data, 2)),
                f64::from(u16_at(data, 4)) / WIDTH_UNITS_PER_CHARACTER,
            )),
            _ => {}
        }
    }

    fn finish(self, name: &str) -> Sheet {
        let mut widths = spreadsheet::measure_column_widths(&self.rows, self.columns);
        for (first, last, characters) in self.widths {
            for width in widths.iter_mut().take(last + 1).skip(first) {
                *width = (characters * COLUMN_WIDTH_CHARACTER).round() + COLUMN_WIDTH_PADDING;
            }
        }
        spreadsheet::sheet(name, self.rows, self.columns, widths)
    }
}

fn read_sheet(stream: &[u8], listed: &Listed, globals: &Globals) -> Sheet {
    let mut reading = Reading {
        globals,
        rows: Vec::new(),
        columns: 0,
        widths: Vec::new(),
        pending: None,
    };
    for record in records(stream, listed.start).iter().skip(1) {
        reading.take(record);
    }
    reading.finish(&listed.name)
}

/// Reads an Excel 97–2003 workbook held in memory.
pub fn parse(bytes: &[u8]) -> Result<Workbook> {
    let compound = Compound::open(bytes).map_err(|error| WorkbookError(error.0))?;
    let stream = match compound.stream("Workbook") {
        Ok(Some(stream)) => stream,
        Ok(None) => return fail("The file holds no workbook."),
        Err(error) => return Err(WorkbookError(error.0)),
    };
    let globals = Globals::read(&stream)?;
    let sheets: Vec<Sheet> = globals
        .sheets
        .iter()
        .map(|listed| read_sheet(&stream, listed, &globals))
        .collect();
    if sheets.is_empty() {
        return fail("The workbook has no visible worksheet.");
    }
    Ok(Workbook { sheets })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rk_values_hold_integers_and_doubles_maybe_in_hundredths() {
        assert_eq!(rk_value((42 << 2) | 2), 42.0);
        assert_eq!(rk_value((1234 << 2) | 3), 12.34);
        assert_eq!(rk_value(((-7i32) << 2) as u32 | 2), -7.0);
        let half = (0.5f64.to_bits() >> 32) as u32;
        assert_eq!(rk_value(half), 0.5);
    }

    #[test]
    fn a_shared_string_runs_across_a_continue_record_and_changes_width() {
        // "ab" in single bytes, the record ends, the rest is "cd" in UTF-16 after its own flags.
        let first: &[u8] = &[4, 0, 0, b'a', b'b'];
        let second: &[u8] = &[1, b'c', 0, b'd', 0];
        let parts = [first, second];
        let mut strings = Strings {
            parts: &parts,
            part: 0,
            at: 0,
        };
        assert_eq!(strings.rich(), "abcd");
        assert_eq!(short_string(&[3, 0, b'x', b'y', b'z'], 0, false), "xyz");
        assert_eq!(error_text(0x07), "#DIV/0!");
    }
}
