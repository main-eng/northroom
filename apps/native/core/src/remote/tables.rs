//! A file's tables as a phone shows them. Every kind the app shows as a table (delimited text,
//! a workbook, a database, contacts, events, an archive's listing: `sheets`) is sheets of rows
//! of cells, so a device draws them all with one grid and reads none of the formats.

use crate::sheets::layout::Table;
use crate::sheets::spreadsheet::{Alignment, Cell, Sheet};
use serde::Serialize;
use std::path::Path;

/// The most rows one `rows` message is answered with.
pub const ROWS_MOST: usize = 200;

fn is_false(flag: &bool) -> bool {
    !flag
}

/// One sheet without its rows: its name, its columns and how many rows it has.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Head {
    pub name: String,
    /// Each column's header: its letters, or a database table's column name.
    pub columns: Vec<String>,
    /// Each column's width in points, as the computer shows it.
    pub widths: Vec<f64>,
    pub rows: usize,
}

/// One cell as it is shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub text: String,
    /// `center` or `end`; absent for a cell that sits at the start of its column.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<&'static str>,
    #[serde(skip_serializing_if = "is_false")]
    pub bold: bool,
}

impl Shown {
    fn of(cell: &Cell) -> Self {
        Self {
            text: cell.text.clone(),
            align: match cell.alignment {
                Alignment::Start => None,
                Alignment::Center => Some("center"),
                Alignment::End => Some("end"),
            },
            bold: cell.bold,
        }
    }
}

/// A row's cells by column, `None` for an empty one, without the empty cells it ends in.
pub type Row = Vec<Option<Shown>>;

fn head_of(sheet: &Sheet) -> Head {
    Head {
        name: sheet.name.clone(),
        columns: sheet.column_names.clone(),
        widths: sheet.column_widths.clone(),
        rows: sheet.row_count,
    }
}

/// The sheets of `table`, or `None` for a file that could not be read.
pub fn heads(table: &Table) -> Option<Vec<Head>> {
    match table {
        Table::Sheets(workbook) | Table::Database(workbook) => {
            Some(workbook.sheets.iter().map(head_of).collect())
        }
        Table::Unreadable(_) => None,
    }
}

/// `row` as a device is sent it.
pub fn row_of(row: &[Option<Cell>]) -> Row {
    let filled = row
        .iter()
        .rposition(Option::is_some)
        .map_or(0, |last| last + 1);
    row[..filled]
        .iter()
        .map(|cell| cell.as_ref().map(Shown::of))
        .collect()
}

/// Up to `count` rows of `sheet` from row `from`, of the rows it holds.
pub fn rows_held(sheet: &Sheet, from: usize, count: usize) -> Vec<Row> {
    let start = from.saturating_sub(sheet.first_row).min(sheet.rows.len());
    let end = start.saturating_add(count).min(sheet.rows.len());
    sheet.rows[start..end]
        .iter()
        .map(|row| row_of(row))
        .collect()
}

/// The file at `path` as a table when its name or contents say it is shown as one, and it is
/// no larger than that kind is read; `None` for any other file.
pub fn read(path: &Path, size_bytes: u64) -> Option<Table> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    if crate::sheets::database::is_database(path) {
        return Some(crate::sheets::database::table(path));
    }
    if crate::sheets::workbook::is_workbook_name(&name) {
        return read_workbook(path, &name, size_bytes);
    }
    let delimited = crate::workspace::text_form(&name) == crate::workspace::TextForm::Table;
    if !delimited || size_bytes > crate::ffi::PAGE_MAX_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    Some(Table::Sheets(crate::sheets::spreadsheet::read(
        &name, &text,
    )))
}

/// A workbook, a contacts or events file, or an archive's listing. An archive lists from its
/// index, however large it is.
fn read_workbook(path: &Path, name: &str, size_bytes: u64) -> Option<Table> {
    let extension = Path::new(name)
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let archive = crate::sheets::archive::is_archive_name(&extension);
    if !archive && size_bytes > crate::sheets::workbook::FILE_MAX_BYTES {
        return None;
    }
    match crate::sheets::workbook::read(path) {
        Ok(workbook) => Some(Table::Sheets(workbook)),
        Err(failure) => {
            eprintln!("a phone's file was not read as a table: {}", failure.0);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(text: &str) -> Option<Cell> {
        Some(Cell {
            text: text.to_owned(),
            alignment: Alignment::Start,
            bold: false,
        })
    }

    #[test]
    fn delimited_text_is_one_sheet_read_a_window_of_rows_at_a_time() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("Prices.csv");
        std::fs::write(&path, "Coffee,Price\nMorning Tide,46\nLighthouse,53\n").unwrap();
        let table = read(&path, 40).expect("a csv is a table");
        let heads = heads(&table).expect("it was read");
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].rows, 3);
        let Table::Sheets(workbook) = &table else {
            panic!("delimited text is read whole");
        };
        let rows = rows_held(&workbook.sheets[0], 1, 1);
        assert_eq!(
            serde_json::to_string(&rows).unwrap(),
            r#"[[{"text":"Morning Tide"},{"text":"46","align":"end"}]]"#
        );
        assert_eq!(rows_held(&workbook.sheets[0], 9, 5), Vec::<Row>::new());
    }

    #[test]
    fn a_row_is_sent_without_the_empty_cells_it_ends_in() {
        let row = [cell("a"), None, cell("c"), None, None];
        assert_eq!(row_of(&row).len(), 3);
        assert_eq!(row_of(&[None, None]), Row::new());
    }

    #[test]
    fn a_file_that_is_no_table_is_none() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.txt");
        std::fs::write(&path, "a,b").unwrap();
        assert_eq!(read(&path, 3), None);
    }
}
