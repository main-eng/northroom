//! A spreadsheet for display: sheets of cells with their text, alignment and weight. Delimited
//! text (`.csv`, `.tsv`, `.tab`) is read here into a one-sheet workbook. The delimiter comes
//! from the first line, fields split per RFC 4180, numbers align to the end of their cells, and
//! each column fits its longest text. Excel workbooks are read in `workbook.rs`.

/// How a cell's text sits in its column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alignment {
    Start,
    Center,
    End,
}

/// One cell as it is shown: its text, where it sits and whether it is bold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub text: String,
    pub alignment: Alignment,
    pub bold: bool,
}

/// A spreadsheet file read for display: an Excel workbook's sheets, or delimited text as one.
#[derive(Clone, Debug, PartialEq)]
pub struct Workbook {
    pub sheets: Vec<Sheet>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sheet {
    pub name: String,
    pub column_count: usize,
    pub column_widths: Vec<f64>,
    /// Each column's header: its letters (`A`, `B`, … `AA`), or a database table's column name.
    pub column_names: Vec<String>,
    /// Each row's cells by column, `None` for an empty field. Rows may be short. A file read
    /// whole holds every row; a database table holds only the rows in view, or none.
    pub rows: Vec<Vec<Option<Cell>>>,
    /// Where `rows` starts among the sheet's rows.
    pub first_row: usize,
    /// How many rows the sheet has.
    pub row_count: usize,
}

const BYTE_ORDER_MARK: char = '\u{feff}';
const CHARACTER_WIDTH: f64 = 8.5;
const CELL_PADDING: f64 = 20.0;
/// The narrowest and widest a column can be.
pub const COLUMN_MIN_WIDTH: f64 = 56.0;
pub const COLUMN_MAX_WIDTH: f64 = 360.0;

/// The delimiter a `.csv` uses: whichever of comma, semicolon and tab appears most often
/// outside quotes on its first line, or a comma on a tie.
pub fn detect_csv_delimiter(text: &str) -> char {
    let mut counts = std::collections::HashMap::new();
    let mut quoted = false;
    for character in text.chars() {
        if character == '"' {
            quoted = !quoted;
        } else if !quoted && (character == '\n' || character == '\r') {
            break;
        } else if !quoted {
            *counts.entry(character).or_insert(0usize) += 1;
        }
    }
    let mut best = ',';
    for candidate in [',', ';', '\t'] {
        if counts.get(&candidate).copied().unwrap_or(0) > counts.get(&best).copied().unwrap_or(0) {
            best = candidate;
        }
    }
    best
}

/// Parser state for delimited text: the rows so far, the current row and field, whether it is
/// inside quotes, and whether the current row has begun.
#[derive(Default)]
struct DelimitedRows {
    rows: Vec<Vec<String>>,
    row: Vec<String>,
    field: String,
    quoted: bool,
    started: bool,
}

impl DelimitedRows {
    /// Reads a character inside quotes, given the `next` one. Returns how many characters it
    /// consumed (a doubled quote is one quote and consumes two).
    fn read_quoted(&mut self, character: char, next: Option<char>) -> usize {
        match (character, next) {
            ('"', Some('"')) => {
                self.field.push('"');
                2
            }
            ('"', _) => {
                self.quoted = false;
                1
            }
            _ => {
                self.field.push(character);
                1
            }
        }
    }

    /// Reads a character outside quotes, given the `next` one. Returns how many characters it
    /// consumed (a CRLF consumes two).
    fn read_unquoted(&mut self, character: char, next: Option<char>, delimiter: char) -> usize {
        if character == '"' && self.field.is_empty() {
            self.quoted = true;
            self.started = true;
            return 1;
        }
        if character == delimiter {
            self.row.push(std::mem::take(&mut self.field));
            self.started = true;
            return 1;
        }
        if character == '\n' || character == '\r' {
            self.end_row();
            self.started = false;
            return if character == '\r' && next == Some('\n') {
                2
            } else {
                1
            };
        }
        self.field.push(character);
        self.started = true;
        1
    }

    fn end_row(&mut self) {
        self.row.push(std::mem::take(&mut self.field));
        self.rows.push(std::mem::take(&mut self.row));
    }
}

/// The rows of delimited text: a quoted field may hold the delimiter, line breaks and doubled
/// quotes; lines end in LF, CRLF or CR; a final line break adds no empty row.
pub fn parse_delimited(text: &str, delimiter: char) -> Vec<Vec<String>> {
    let source: Vec<char> = text
        .strip_prefix(BYTE_ORDER_MARK)
        .unwrap_or(text)
        .chars()
        .collect();
    let mut reader = DelimitedRows::default();
    let mut index = 0;
    while index < source.len() {
        let (character, next) = (source[index], source.get(index + 1).copied());
        index += if reader.quoted {
            reader.read_quoted(character, next)
        } else {
            reader.read_unquoted(character, next, delimiter)
        };
    }
    if reader.started {
        reader.end_row();
    }
    reader.rows
}

/// Whether `text` is a hexadecimal, binary or octal literal as JavaScript's `Number` reads it.
fn is_radix_literal(text: &str) -> bool {
    let (Some(prefix), Some(digits)) = (text.get(..2), text.get(2..)) else {
        return false;
    };
    let is_digit: fn(char) -> bool = match prefix {
        "0x" | "0X" => |c| c.is_ascii_hexdigit(),
        "0b" | "0B" => |c| c == '0' || c == '1',
        "0o" | "0O" => |c| ('0'..='7').contains(&c),
        _ => return false,
    };
    !digits.is_empty() && digits.chars().all(is_digit)
}

/// Whether `text` is a decimal with an optional sign, point and exponent.
fn is_decimal(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match body.find(['e', 'E']) {
        Some(at) => (&body[..at], Some(&body[at + 1..])),
        None => (body, None),
    };
    let (whole, fraction) = match mantissa.find('.') {
        Some(at) => (&mantissa[..at], Some(&mantissa[at + 1..])),
        None => (mantissa, None),
    };
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    let mantissa_ok = digits(whole)
        && fraction.is_none_or(digits)
        && !(whole.is_empty() && fraction.is_none_or(str::is_empty));
    let exponent_ok = exponent.is_none_or(|part| {
        let part = part.strip_prefix(['+', '-']).unwrap_or(part);
        !part.is_empty() && digits(part)
    });
    mantissa_ok && exponent_ok
}

/// Whether JavaScript's `Number(text.trim())` is a finite number: a decimal with an optional
/// sign, point and exponent, or a hexadecimal, binary or octal literal.
fn is_numeric(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && (is_radix_literal(trimmed) || is_decimal(trimmed))
}

/// Column widths that fit a sheet's text: each column fits its longest text, counted in UTF-16
/// units, between `COLUMN_MIN_WIDTH` and `COLUMN_MAX_WIDTH`.
pub fn measure_column_widths(rows: &[Vec<Option<Cell>>], column_count: usize) -> Vec<f64> {
    let mut longest = vec![0usize; column_count];
    for row in rows {
        for (column, cell) in row.iter().enumerate().take(column_count) {
            if let Some(cell) = cell {
                longest[column] = longest[column].max(cell.text.encode_utf16().count());
            }
        }
    }
    longest
        .into_iter()
        .map(|characters| {
            ((characters as f64 * CHARACTER_WIDTH).ceil() + CELL_PADDING)
                .clamp(COLUMN_MIN_WIDTH, COLUMN_MAX_WIDTH)
        })
        .collect()
}

/// A sheet named `name` with these rows and columns sized to fit them.
pub fn sheet(
    name: &str,
    rows: Vec<Vec<Option<Cell>>>,
    column_count: usize,
    column_widths: Vec<f64>,
) -> Sheet {
    Sheet {
        name: name.to_owned(),
        column_count,
        column_widths,
        column_names: (0..column_count).map(column_name).collect(),
        first_row: 0,
        row_count: rows.len(),
        rows,
    }
}

/// Delimited text as one sheet named `name`.
pub fn build(name: &str, text: &str, delimiter: char) -> Sheet {
    let rows: Vec<Vec<Option<Cell>>> = parse_delimited(text, delimiter)
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|text| {
                    (!text.is_empty()).then(|| Cell {
                        alignment: if is_numeric(&text) {
                            Alignment::End
                        } else {
                            Alignment::Start
                        },
                        bold: false,
                        text,
                    })
                })
                .collect()
        })
        .collect();
    let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths = measure_column_widths(&rows, column_count);
    sheet(name, rows, column_count, widths)
}

/// Whether a file's extension, in lower case, is a tab-separated table's: `.tsv` or `.tab`.
pub fn is_tab_separated(extension: &str) -> bool {
    matches!(extension, "tsv" | "tab")
}

/// Whether a file's extension, in lower case, is a delimited table's: `.csv` or a tab-separated
/// one.
pub fn is_delimited(extension: &str) -> bool {
    extension == "csv" || is_tab_separated(extension)
}

/// The workbook for a delimited file named `name`: tab-separated for `.tsv` and `.tab`, and for
/// `.csv` whatever its first line uses. Its one sheet is named after the file.
pub fn read(name: &str, text: &str) -> Workbook {
    let extension = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let delimiter = if is_tab_separated(&extension) {
        '\t'
    } else {
        detect_csv_delimiter(text)
    };
    Workbook {
        sheets: vec![build(name, text, delimiter)],
    }
}

/// The most columns a sheet draws.
pub const SHOWN_COLUMNS: usize = 200;
/// Shown in place of a workbook that cannot be read.
pub const UNREADABLE: &str = "Couldn’t read this workbook. It may be damaged or not an Excel file.";
/// Shown for a sheet with no rows.
pub const EMPTY_SHEET: &str = "This sheet is empty.";
/// Shown for an unreadable database, a database with no tables, and a table with no rows.
pub const UNREADABLE_DATABASE: &str =
    "Couldn’t read this database. It may be damaged or not a SQLite file.";
pub const EMPTY_DATABASE: &str = "This database has no tables.";
pub const EMPTY_TABLE: &str = "This table is empty.";

/// A cell as drawn in its row, with the number of columns it spans.
#[derive(Debug, PartialEq)]
pub struct ShownCell<'a> {
    pub cell: &'a Cell,
    /// How many columns the cell spans. Start-aligned text runs over the empty cells after it;
    /// anything else takes one.
    pub span: usize,
}

/// One row's cells as shown across `columns` columns.
pub fn shown_row(row: &[Option<Cell>], columns: usize) -> Vec<Option<ShownCell<'_>>> {
    let at = |column: usize| row.get(column).and_then(Option::as_ref);
    let mut shown = Vec::with_capacity(columns.min(row.len()));
    let mut column = 0;
    while column < columns.min(row.len()) {
        let Some(cell) = at(column) else {
            shown.push(None);
            column += 1;
            continue;
        };
        let mut span = 1;
        if cell.alignment == Alignment::Start {
            while column + span < columns && at(column + span).is_none() {
                span += 1;
            }
        }
        shown.push(Some(ShownCell { cell, span }));
        shown.extend(
            (1..span)
                .map(|_| None)
                .take(row.len().saturating_sub(column + 1)),
        );
        column += span;
    }
    shown
}

/// A column's name: 0 is `A`, 25 is `Z`, 26 is `AA`.
pub fn column_name(index: usize) -> String {
    let mut name = Vec::new();
    let mut rest = index as i64;
    while rest >= 0 {
        name.push((b'A' + (rest % 26) as u8) as char);
        rest = rest / 26 - 1;
    }
    name.iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(text: &str, alignment: Alignment) -> Option<Cell> {
        Some(Cell {
            text: text.into(),
            alignment,
            bold: false,
        })
    }

    #[test]
    fn a_starting_text_runs_over_the_empty_cells_after_it() {
        let row = vec![
            text("a long title", Alignment::Start),
            None,
            None,
            text("4", Alignment::End),
            None,
            text("x", Alignment::Center),
            None,
        ];
        let spans: Vec<Option<usize>> = shown_row(&row, 7)
            .iter()
            .map(|cell| cell.as_ref().map(|cell| cell.span))
            .collect();
        assert_eq!(spans, [Some(3), None, None, Some(1), None, Some(1), None]);
        // A text at the row's end runs on to the last column shown, past the row's own end.
        let ending = [None, text("note", Alignment::Start)];
        let last = shown_row(&ending, 5);
        assert_eq!(last.len(), 2);
        assert_eq!(last[1].as_ref().map(|cell| cell.span), Some(4));
        // Only the columns shown count.
        assert_eq!(shown_row(&row, 2).len(), 2);
        assert_eq!(
            shown_row(&row, 2)[0].as_ref().map(|cell| cell.span),
            Some(2)
        );
    }

    /// Delimited text, with JavaScript's number parsing deciding alignment.
    #[test]
    fn reads_delimited_text_and_aligns_numbers_to_the_end() {
        assert_eq!(detect_csv_delimiter("a;b;c\n1,5;2,5;3\n"), ';');
        assert_eq!(detect_csv_delimiter("\"a,b\";c\n"), ';');
        assert_eq!(detect_csv_delimiter("one\n"), ',');
        assert_eq!(
            parse_delimited(
                "name,\"note, with comma\"\r\n\"say \"\"hi\"\"\",\"two\nlines\"\n",
                ','
            ),
            [
                vec!["name", "note, with comma"],
                vec!["say \"hi\"", "two\nlines"]
            ]
        );
        assert_eq!(
            parse_delimited("\u{feff}a\rb\r\n", ','),
            [vec!["a"], vec!["b"]]
        );
        assert_eq!(
            parse_delimited("a,\n,b", ','),
            [vec!["a", ""], vec!["", "b"]]
        );
        for numeric in [
            "1", " 2.5 ", "-3", "+4", ".5", "5.", "1e3", "2E-2", "0x1F", "0b11", "0o7",
        ] {
            assert!(is_numeric(numeric), "{numeric}");
        }
        for text in [
            "", " ", "Infinity", "1,5", "12px", "e5", ".", "-", "1e", "0x", "--1", "0xG", "00ff",
            "x012", "xx12", "bb01", "o0o7",
        ] {
            assert!(!is_numeric(text), "{text}");
        }
        let workbook = read(
            "data.csv",
            "Name,Value\nalpha,1\nA much longer name than the rest,22\n",
        );
        let sheet = &workbook.sheets[0];
        assert_eq!(sheet.name, "data.csv");
        assert_eq!(sheet.column_count, 2);
        // 32 letters, and "Value" (5) wider than the minimum.
        assert_eq!(
            sheet.column_widths,
            [(32.0f64 * 8.5).ceil() + 20.0, (5.0f64 * 8.5).ceil() + 20.0]
        );
        assert_eq!(read("x.csv", "a\n").sheets[0].column_widths, [56.0]);
        assert_eq!(
            sheet.rows[1][1],
            Some(Cell {
                text: "1".into(),
                alignment: Alignment::End,
                bold: false
            })
        );
        assert_eq!(
            sheet.rows[0][0],
            Some(Cell {
                text: "Name".into(),
                alignment: Alignment::Start,
                bold: false
            })
        );
        assert_eq!(read("data.tsv", "a,b\tc\n").sheets[0].rows[0].len(), 2);
        assert_eq!(
            (
                column_name(0),
                column_name(25),
                column_name(26),
                column_name(701),
                column_name(702)
            ),
            (
                "A".into(),
                "Z".into(),
                "AA".into(),
                "ZZ".into(),
                "AAA".into()
            )
        );
    }
}
