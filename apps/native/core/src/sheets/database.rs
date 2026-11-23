//! SQLite database files on the drive. The view shows each table as a sheet (`sheets::layout`)
//! and reads rows a window at a time, so a table of millions of rows opens as fast as one of
//! ten. The card shows the top of the first table. A page can query a database in its own
//! folder with `window.__nr.sql` (`ask`, via `page::host`). The engine is the system SQLite
//! (`sqlite`).
//!
//! A file is a database if it starts with SQLite's 16-byte header, whatever its name. SQLite
//! keeps companion files beside it while writing (`-journal`, `-wal`, `-shm`). They belong to
//! the database: a folder lists them only when showing hidden files, and they move with it
//! (`workspace`).

use crate::sheets::layout::{Held, Table};
use crate::sheets::spreadsheet::{Alignment, Cell, Sheet, Workbook};
pub use crate::sheets::sqlite::Access;
use crate::sheets::sqlite::{Connection, Failure, Statement, Value};
use crate::Relock;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// The header every SQLite database file starts with.
const HEADER: &[u8; 16] = b"SQLite format 3\0";
/// Extensions that mark an empty file as a not-yet-written database.
const EXTENSIONS: [&str; 4] = ["sqlite", "sqlite3", "db", "db3"];
/// Suffixes of SQLite's companion files: rollback journal, write-ahead log, and the log's
/// shared-memory index.
const COMPANIONS: [&str; 3] = ["-journal", "-wal", "-shm"];
/// Companions copied with a database. `-shm` is left out because SQLite rebuilds it from the
/// log.
const COPIED_COMPANIONS: [&str; 2] = ["-journal", "-wal"];

/// Busy timeout for each view read.
const VIEW_PATIENCE: Duration = Duration::from_secs(5);
/// Time limit for all of one page call's statements together.
const PAGE_PATIENCE: Duration = Duration::from_secs(10);
/// Maximum size of one JSON answer to a page.
const ANSWER_MAX_BYTES: usize = 32 * 1024 * 1024;

/// Rows measured from each end of a table to size its columns. The last rows tend to hold the
/// longest ids and latest dates.
const MEASURED_ROWS: usize = 200;
/// Extra rows read on each side of the visible ones, so scrolling does not hit the file every
/// frame.
const ROWS_AROUND: usize = 200;
/// How many characters of text and bytes of a blob a cell shows.
const SHOWN_CHARACTERS: usize = 400;
const SHOWN_BYTES: usize = 16;
/// Rows of the first table shown on a card, and the maximum characters per value.
const CARD_ROWS: usize = 24;
const CARD_CHARACTERS: usize = 18;
/// Maximum remembered (row index, row id) anchors per sheet; all are dropped past this.
const ANCHORS_KEPT: usize = 512;

/// Whether `path` is a SQLite database: it starts with the SQLite header, or is empty with a
/// database extension.
pub fn is_database(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let Ok(stat) = file.metadata() else {
        return false;
    };
    if !stat.is_file() {
        return false;
    }
    if stat.len() == 0 {
        return path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                EXTENSIONS
                    .iter()
                    .any(|known| extension.eq_ignore_ascii_case(known))
            });
    }
    let mut header = [0u8; 16];
    file.read_exact(&mut header).is_ok() && header == *HEADER
}

/// Whether `name` in `folder` is a companion file sitting beside its database.
pub fn is_companion(folder: &Path, name: &str) -> bool {
    COMPANIONS
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .is_some_and(|database| !database.is_empty() && is_database(&folder.join(database)))
}

/// Companion files beside `path`, each with its suffix. Found even if `path` does not exist:
/// leftovers from a database moved or deleted without them, which a new database at `path`
/// would adopt.
pub fn companions(path: &Path) -> Vec<(&'static str, PathBuf)> {
    present(path, &COMPANIONS)
}

/// Companion files beside `path` that a copy should include.
pub fn copied_companions(path: &Path) -> Vec<(&'static str, PathBuf)> {
    present(path, &COPIED_COMPANIONS)
}

fn present(path: &Path, suffixes: &[&'static str]) -> Vec<(&'static str, PathBuf)> {
    suffixes
        .iter()
        .map(|suffix| (*suffix, beside(path, suffix)))
        .filter(|(_, file)| file.symlink_metadata().is_ok())
        .collect()
}

/// `path` with `suffix` appended to its file name.
pub fn beside(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// A database's on-disk version: size and modification time of the file and its write-ahead
/// log, which holds the newest rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version([Option<(u64, Option<SystemTime>)>; 2]);

impl Version {
    pub fn of(path: &Path) -> Self {
        let stat = |file: &Path| {
            std::fs::metadata(file)
                .ok()
                .map(|stat| (stat.len(), stat.modified().ok()))
        };
        Self([stat(path), stat(&beside(path, "-wal"))])
    }
}

// MARK: View reads

/// Quotes `name` as an SQL identifier.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// How the view queries one table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Shape {
    /// Quoted table name.
    table: String,
    /// The select list, truncating long text and blobs to what a cell shows.
    columns: String,
    /// An unshadowed alias for the row id, if any. Rows are then seeked by row id, so a window
    /// deep in the table costs the same as one at the top.
    key: Option<&'static str>,
}

impl Shape {
    fn of(connection: &Connection, name: &str) -> Result<(Self, Vec<String>), Failure> {
        let table = quoted(name);
        let names = connection
            .prepare(&format!("SELECT * FROM {table} LIMIT 0"))?
            .column_names();
        let columns = names
            .iter()
            .map(|column| {
                let column = quoted(column);
                // One past the shown length, to detect truncation.
                format!(
                    "CASE typeof({column}) WHEN 'text' THEN substr({column}, 1, {}) \
                     WHEN 'blob' THEN substr({column}, 1, {}) ELSE {column} END",
                    SHOWN_CHARACTERS + 1,
                    SHOWN_BYTES + 1
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let key = ["rowid", "_rowid_", "oid"].into_iter().find(|key| {
            !names.iter().any(|column| column.eq_ignore_ascii_case(key))
                && connection
                    .prepare(&format!("SELECT {key} FROM {table} LIMIT 0"))
                    .is_ok()
        });
        Ok((
            Self {
                table,
                columns,
                key,
            },
            names,
        ))
    }

    /// Rows `first..first + count`, read from the nearest earlier row with a known row id
    /// (`anchors` maps row index to row id). The last row read becomes a new anchor.
    fn rows(
        &self,
        connection: &Connection,
        anchors: &mut BTreeMap<usize, i64>,
        first: usize,
        count: usize,
    ) -> Result<Vec<Vec<Option<Cell>>>, Failure> {
        let Self {
            table,
            columns,
            key,
        } = self;
        let integer = |number: usize| Value::Integer(i64::try_from(number).unwrap_or(i64::MAX));
        let Some(key) = key else {
            let mut statement =
                connection.prepare(&format!("SELECT {columns} FROM {table} LIMIT ? OFFSET ?"))?;
            statement.bind(&[integer(count), integer(first)])?;
            let mut rows = Vec::new();
            while statement.step()? {
                rows.push(statement.row().into_iter().map(cell).collect());
            }
            return Ok(rows);
        };
        let anchor = anchors
            .range(..first)
            .next_back()
            .map(|(at, id)| (*at, *id));
        let mut statement = match anchor {
            Some((at, id)) => {
                let mut statement = connection.prepare(&format!(
                    "SELECT {key}, {columns} FROM {table} WHERE {key} > ? ORDER BY {key} \
                     LIMIT ? OFFSET ?"
                ))?;
                statement.bind(&[Value::Integer(id), integer(count), integer(first - at - 1)])?;
                statement
            }
            None => {
                let mut statement = connection.prepare(&format!(
                    "SELECT {key}, {columns} FROM {table} ORDER BY {key} LIMIT ? OFFSET ?"
                ))?;
                statement.bind(&[integer(count), integer(first)])?;
                statement
            }
        };
        let mut rows = Vec::new();
        let mut last = None;
        while statement.step()? {
            let mut values = statement.row().into_iter();
            if let Some(Value::Integer(id)) = values.next() {
                last = Some(id);
            }
            rows.push(values.map(cell).collect());
        }
        if let (Some(id), false) = (last, rows.is_empty()) {
            if anchors.len() >= ANCHORS_KEPT {
                anchors.clear();
            }
            anchors.insert(first + rows.len() - 1, id);
        }
        Ok(rows)
    }
}

impl Shape {
    /// The last `count` rows of a table with a row id. Empty for tables without one, whose end
    /// can only be reached by scanning.
    fn last_rows(
        &self,
        connection: &Connection,
        count: usize,
    ) -> Result<Vec<Vec<Option<Cell>>>, Failure> {
        let Self {
            table,
            columns,
            key: Some(key),
        } = self
        else {
            return Ok(Vec::new());
        };
        let mut statement = connection.prepare(&format!(
            "SELECT {columns} FROM {table} ORDER BY {key} DESC LIMIT ?"
        ))?;
        statement.bind(&[Value::Integer(i64::try_from(count).unwrap_or(i64::MAX))])?;
        let mut rows = Vec::new();
        while statement.step()? {
            rows.push(statement.row().into_iter().map(cell).collect());
        }
        Ok(rows)
    }
}

/// Formats a real number, always with a decimal point so it does not read as an integer.
fn real_text(number: f64) -> String {
    let text = if number != 0.0 && !(1e-5..1e15).contains(&number.abs()) {
        format!("{number:e}")
    } else {
        number.to_string()
    };
    if text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'-')
    {
        text + ".0"
    } else {
        text
    }
}

/// A value as a cell: numbers right-aligned, truncated text ending in an ellipsis, blobs as
/// SQL literals (`x'89504e47…'`), and `None` for NULL, empty text or an empty blob.
fn cell(value: Value) -> Option<Cell> {
    let (text, alignment) = match value {
        Value::Null => return None,
        Value::Integer(number) => (number.to_string(), Alignment::End),
        Value::Real(number) => (real_text(number), Alignment::End),
        Value::Text(text) => {
            let mut shown: String = text.chars().take(SHOWN_CHARACTERS).collect();
            if shown.len() < text.len() {
                shown.push('…');
            }
            (shown, Alignment::Start)
        }
        Value::Blob(bytes) if bytes.is_empty() => return None,
        Value::Blob(bytes) => {
            let mut shown = String::from("x'");
            for byte in bytes.iter().take(SHOWN_BYTES) {
                let _ = write!(shown, "{byte:02x}");
            }
            shown.push_str(if bytes.len() > SHOWN_BYTES {
                "…'"
            } else {
                "'"
            });
            (shown, Alignment::Start)
        }
    };
    (!text.is_empty()).then_some(Cell {
        text,
        alignment,
        bold: false,
    })
}

/// A column name as a card cell.
fn name_cell(name: &str) -> Option<Cell> {
    Some(Cell {
        text: name.to_owned(),
        alignment: Alignment::Start,
        bold: false,
    })
}

/// User table names in creation order, excluding SQLite's internal tables.
fn table_names(connection: &Connection) -> Result<Vec<String>, Failure> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' \
         AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY rowid",
    )?;
    let mut names = Vec::new();
    while statement.step()? {
        if let Value::Text(name) = statement.value(0) {
            names.push(name);
        }
    }
    Ok(names)
}

fn row_count(connection: &Connection, shape: &Shape) -> Result<usize, Failure> {
    let mut statement = connection.prepare(&format!("SELECT count(*) FROM {}", shape.table))?;
    statement.step()?;
    Ok(match statement.value(0) {
        Value::Integer(count) => usize::try_from(count).unwrap_or(0),
        _ => 0,
    })
}

/// Reads the database for the view: one empty sheet per table, with column widths fitted to
/// the names and the first and last rows (`sheets::layout::fitting_widths`). Rows load as they
/// scroll into view. Unreadable tables, such as virtual tables whose module is missing, are
/// skipped.
fn read(
    connection: &Connection,
    measure: &mut crate::dialog::layout::Measure,
) -> Result<(Workbook, Vec<Shape>), Failure> {
    let mut sheets = Vec::new();
    let mut shapes = Vec::new();
    for name in table_names(connection)? {
        let Ok((shape, columns)) = Shape::of(connection, &name) else {
            continue;
        };
        let count = row_count(connection, &shape)?;
        let mut measured = shape.rows(connection, &mut BTreeMap::new(), 0, MEASURED_ROWS)?;
        measured.extend(shape.last_rows(connection, MEASURED_ROWS)?);
        let widths = crate::sheets::layout::fitting_widths(&columns, &measured, measure);
        sheets.push(Sheet {
            name,
            column_count: columns.len(),
            column_widths: widths,
            column_names: columns,
            rows: Vec::new(),
            first_row: 0,
            row_count: count,
        });
        shapes.push(shape);
    }
    Ok((Workbook { sheets }, shapes))
}

/// The last rows read: sheet, first row index, rows.
type Window = (usize, usize, Vec<Vec<Option<Cell>>>);

/// Cached read of the open database, valid while the file is unchanged.
struct Reading {
    path: PathBuf,
    version: Version,
    table: Table,
    shapes: Vec<Shape>,
    /// Per sheet, row index to known row id.
    anchors: Vec<BTreeMap<usize, i64>>,
    window: Option<Window>,
}

impl Reading {
    fn of(path: &Path) -> Self {
        let version = Version::of(path);
        let read = Connection::open(path, Access::Reads, VIEW_PATIENCE)
            .and_then(|connection| read(&connection, &mut crate::text::typeset::width));
        let (table, shapes) = match read {
            Ok((workbook, shapes)) => (Table::Database(workbook), shapes),
            Err(failure) => {
                eprintln!("the database was not read: {failure}");
                (Table::Unreadable(Held::Database), Vec::new())
            }
        };
        Self {
            path: path.to_owned(),
            version,
            table,
            anchors: vec![BTreeMap::new(); shapes.len()],
            shapes,
            window: None,
        }
    }

    /// Rows of `sheet`, served from the cached window when it covers them.
    fn rows(&mut self, sheet: usize, rows: Range<usize>) -> Vec<Vec<Option<Cell>>> {
        let count = match &self.table {
            Table::Database(workbook) => {
                workbook.sheets.get(sheet).map_or(0, |open| open.row_count)
            }
            Table::Sheets(_) | Table::Unreadable(_) => 0,
        };
        let rows = rows.start.min(count)..rows.end.min(count);
        if rows.is_empty() {
            return Vec::new();
        }
        let held = self.window.as_ref().is_some_and(|(of, first, held)| {
            *of == sheet && *first <= rows.start && rows.end <= first + held.len()
        });
        if !held {
            let first = rows.start.saturating_sub(ROWS_AROUND);
            let last = (rows.end + ROWS_AROUND).min(count);
            let read =
                Connection::open(&self.path, Access::Reads, VIEW_PATIENCE).and_then(|connection| {
                    self.shapes[sheet].rows(
                        &connection,
                        &mut self.anchors[sheet],
                        first,
                        last - first,
                    )
                });
            match read {
                Ok(read) => self.window = Some((sheet, first, read)),
                Err(failure) => {
                    eprintln!("the database's rows were not read: {failure}");
                    return Vec::new();
                }
            }
        }
        let Some((_, first, held)) = &self.window else {
            return Vec::new();
        };
        // A table that shrank since it was counted returns fewer rows than asked.
        let from = (rows.start - first).min(held.len());
        let to = (rows.end - first).min(held.len());
        held[from..to].to_vec()
    }
}

static READING: Mutex<Option<Reading>> = Mutex::new(None);

/// Runs `action` on the cached reading, rereading if the path or file version changed.
fn with_reading<T>(path: &Path, action: impl FnOnce(&mut Reading) -> T) -> T {
    let mut reading = READING.relock();
    let current = reading
        .as_ref()
        .is_some_and(|held| held.path == path && held.version == Version::of(path));
    if !current {
        *reading = Some(Reading::of(path));
    }
    action(reading.as_mut().expect("a reading was just made"))
}

/// The view's table for `path` (`Table::Database` with empty sheets), or `Table::Unreadable`.
/// Read once per file version.
pub fn table(path: &Path) -> Table {
    with_reading(path, |reading| reading.table.clone())
}

/// Visible rows of `sheet` for the grid. Fewer if the table is shorter, none if unreadable.
pub fn rows(path: &Path, sheet: usize, rows: Range<usize>) -> Vec<Vec<Option<Cell>>> {
    with_reading(path, |reading| reading.rows(sheet, rows))
}

/// The top of the first table as monospace text for the card: a header line, then one line per
/// row, each column padded to its longest (truncated) value with two spaces between. Empty if
/// there is no table or it cannot be read.
pub fn card_text(path: &Path) -> String {
    let read = Connection::open(path, Access::Reads, VIEW_PATIENCE).and_then(|connection| {
        let Some(name) = table_names(&connection)?.into_iter().next() else {
            return Ok(Vec::new());
        };
        let (shape, columns) = Shape::of(&connection, &name)?;
        let mut lines = vec![columns.iter().map(|column| name_cell(column)).collect()];
        lines.extend(shape.rows(&connection, &mut BTreeMap::new(), 0, CARD_ROWS)?);
        Ok(lines)
    });
    let lines: Vec<Vec<String>> = read
        .unwrap_or_default()
        .iter()
        .map(|line: &Vec<Option<Cell>>| line.iter().map(card_value).collect())
        .collect();
    let columns = lines.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            lines
                .iter()
                .filter_map(|line| line.get(column))
                .map(|value| value.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    lines
        .iter()
        .map(|line| {
            line.iter()
                .zip(&widths)
                .map(|(value, width)| format!("{value:<width$}"))
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A cell's text for a card line: newlines become spaces, truncated to the card width.
fn card_value(cell: &Option<Cell>) -> String {
    let text = cell.as_ref().map_or("", |cell| cell.text.as_str());
    let mut shown: String = text
        .chars()
        .take(CARD_CHARACTERS)
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if shown.len() < text.len() {
        shown.pop();
        shown.push('…');
    }
    shown
}

// MARK: Page queries

/// One statement from a page, with values for its `?` parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub sql: String,
    pub values: Vec<Value>,
}

/// Parses a page call's statements: `[[sql, [value, …]], …]`, where each value is a number,
/// string, boolean or null.
pub fn asked(run: &serde_json::Value) -> Result<Vec<Asked>, String> {
    let malformed = || "the call is not [[sql, [values]], …]".to_owned();
    let statements = run.as_array().ok_or_else(malformed)?;
    statements
        .iter()
        .map(|statement| {
            let [sql, values] = statement.as_array().ok_or_else(malformed)?.as_slice() else {
                return Err(malformed());
            };
            let values = values
                .as_array()
                .ok_or_else(malformed)?
                .iter()
                .map(value_of)
                .collect::<Result<_, _>>()?;
            Ok(Asked {
                sql: sql.as_str().ok_or_else(malformed)?.to_owned(),
                values,
            })
        })
        .collect()
}

fn value_of(value: &serde_json::Value) -> Result<Value, String> {
    use serde_json::Value as Json;
    match value {
        Json::Null => Ok(Value::Null),
        Json::Bool(flag) => Ok(Value::Integer(i64::from(*flag))),
        Json::String(text) => Ok(Value::Text(text.clone())),
        Json::Number(number) => Ok(number.as_i64().map_or_else(
            || Value::Real(number.as_f64().unwrap_or(f64::NAN)),
            Value::Integer,
        )),
        Json::Array(_) | Json::Object(_) => {
            Err("a value is a number, a text, true, false or null".to_owned())
        }
    }
}

/// `text` as a JSON string literal.
fn json_text(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

/// Writes `value` as JSON: number, string or null; blobs as `{"base64": "…"}`; non-finite reals
/// as null.
fn json_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Integer(number) => {
            let _ = write!(out, "{number}");
        }
        Value::Real(number) => match serde_json::Number::from_f64(*number) {
            Some(number) => {
                let _ = write!(out, "{number}");
            }
            None => out.push_str("null"),
        },
        Value::Text(text) => out.push_str(&json_text(text)),
        Value::Blob(bytes) => {
            let _ = write!(
                out,
                "{{\"base64\":\"{}\"}}",
                crate::update::sha512::base64(bytes)
            );
        }
    }
}

/// Runs one statement and writes `{"columns": [name], "rows": [{name: value}], "changes": n,
/// "lastInsertRowid": id}` to `out`, with row values in column order. A read-only statement
/// reports no changes and no insert.
fn answer(connection: &Connection, asked: &Asked, out: &mut String) -> Result<(), Failure> {
    let mut statement = connection.prepare(&asked.sql)?;
    statement.bind(&asked.values)?;
    let names: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| json_text(name))
        .collect();
    let _ = write!(out, "{{\"columns\":[{}],\"rows\":[", names.join(","));
    let mut first = true;
    while statement.step()? {
        if !first {
            out.push(',');
        }
        first = false;
        write_answer_row(&statement, &names, out);
        if out.len() > ANSWER_MAX_BYTES {
            return Err(Failure::Said(
                "the answer is larger than 32 MB; ask for fewer rows".to_owned(),
            ));
        }
    }
    let (changes, put) = if statement.only_reads() {
        (0, 0)
    } else {
        (connection.changes(), connection.last_insert_rowid())
    };
    let _ = write!(out, "],\"changes\":{changes},\"lastInsertRowid\":{put}}}");
    Ok(())
}

/// One result row, with JSON-quoted column names in the statement's order.
fn write_answer_row(statement: &Statement<'_>, names: &[String], out: &mut String) {
    out.push('{');
    for (column, name) in names.iter().enumerate() {
        if column > 0 {
            out.push(',');
        }
        out.push_str(name);
        out.push(':');
        json_value(&statement.value(column), out);
    }
    out.push('}');
}

/// Runs several statements in one transaction. Closing on an error rolls them all back.
fn answer_statements(
    connection: &Connection,
    statements: &[Asked],
    access: Access,
) -> Result<String, Failure> {
    let together = statements.len() > 1;
    if together {
        // Readers get one snapshot; writers take the write lock before any statement runs.
        connection.run(match access {
            Access::Reads => "BEGIN",
            Access::Writes | Access::Creates => "BEGIN IMMEDIATE",
        })?;
    }
    let mut out = String::from("{\"results\":[");
    for (index, asked) in statements.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        answer(connection, asked, &mut out)?;
    }
    if together {
        connection.run("COMMIT")?;
    }
    out.push_str("]}");
    Ok(out)
}

/// Answers a page's call on `file` as `{"results": [answer]}`, one `answer` per statement.
/// Multiple statements run in one transaction, all or nothing. Errors carry SQLite's message.
/// A missing database is created by the first writing call (`Access::Creates`); a call that
/// only reads, or fails, leaves no empty file behind.
pub fn ask(file: &Path, statements: &[Asked], access: Access) -> Result<String, String> {
    let made = access == Access::Creates && !file.exists();
    // Closing a connection mid-transaction rolls it back.
    let answer = Connection::open(file, access, PAGE_PATIENCE)
        .and_then(|connection| answer_statements(&connection, statements, access))
        .map_err(|failure| failure.to_string());
    if made && std::fs::metadata(file).is_ok_and(|stat| stat.len() == 0) {
        let _ = std::fs::remove_file(file);
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATIENT: Duration = Duration::from_secs(30);

    fn made(folder: &Path, name: &str, statements: &[&str]) -> PathBuf {
        let file = folder.join(name);
        let connection = Connection::open(&file, Access::Creates, PATIENT).expect("open");
        for statement in statements {
            connection.run(statement).expect(statement);
        }
        file
    }

    fn texts(rows: &[Vec<Option<Cell>>]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|row| {
                row.iter()
                    .map(|cell| {
                        cell.as_ref()
                            .map_or(String::new(), |cell| cell.text.clone())
                    })
                    .collect()
            })
            .collect()
    }

    /// Column widths of `sheet`, measuring every character as 7 pt.
    fn measured_widths(file: &Path, sheet: usize) -> Vec<f64> {
        let connection = Connection::open(file, Access::Reads, PATIENT).expect("open");
        let mut sevens = |text: &str, _| text.chars().count() as f64 * 7.0;
        let (workbook, _) = read(&connection, &mut sevens).expect("read");
        workbook.sheets[sheet].column_widths.clone()
    }

    fn sheets(table: Table) -> Vec<Sheet> {
        match table {
            Table::Database(workbook) => workbook.sheets,
            other => panic!("not a database: {other:?}"),
        }
    }

    #[test]
    fn a_database_is_told_by_its_header_and_an_empty_file_by_its_name() {
        let folder = tempfile::tempdir().expect("folder");
        let real = made(folder.path(), "People", &["CREATE TABLE t (i)"]);
        assert!(is_database(&real));
        let words = folder.path().join("words.sqlite");
        std::fs::write(&words, "not a database, whatever it is called").unwrap();
        assert!(!is_database(&words));
        let empty = folder.path().join("new.DB");
        std::fs::write(&empty, "").unwrap();
        assert!(is_database(&empty));
        let nothing = folder.path().join("empty.txt");
        std::fs::write(&nothing, "").unwrap();
        assert!(!is_database(&nothing));
        assert!(!is_database(folder.path()));
        assert!(!is_database(&folder.path().join("missing.sqlite")));
    }

    #[test]
    fn a_companion_is_a_file_sqlite_keeps_beside_a_database() {
        let folder = tempfile::tempdir().expect("folder");
        made(folder.path(), "app.sqlite", &["CREATE TABLE t (i)"]);
        std::fs::write(folder.path().join("trip"), "words").unwrap();
        for name in ["app.sqlite-journal", "app.sqlite-wal", "app.sqlite-shm"] {
            assert!(is_companion(folder.path(), name), "{name}");
        }
        // Next to a non-database file, or nothing, these names are ordinary files.
        for name in [
            "trip-journal",
            "gone.sqlite-wal",
            "-wal",
            "app.sqlite",
            "app.sqlite-old",
        ] {
            assert!(!is_companion(folder.path(), name), "{name}");
        }
    }

    #[test]
    fn the_companions_beside_a_name_are_found_and_a_copy_takes_all_but_the_index() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "app.sqlite", &["CREATE TABLE t (i)"]);
        assert!(companions(&file).is_empty());
        for suffix in ["-wal", "-shm"] {
            std::fs::write(beside(&file, suffix), "x").unwrap();
        }
        let found: Vec<&str> = companions(&file)
            .iter()
            .map(|(suffix, _)| *suffix)
            .collect();
        assert_eq!(found, ["-wal", "-shm"]);
        assert_eq!(
            copied_companions(&file),
            [("-wal", folder.path().join("app.sqlite-wal"))]
        );
        // Leftovers are found even when the database itself is gone.
        std::fs::write(folder.path().join("gone.sqlite-journal"), "x").unwrap();
        assert_eq!(companions(&folder.path().join("gone.sqlite")).len(), 1);
    }

    #[test]
    fn each_table_is_a_sheet_with_its_columns_named_and_measured_and_its_rows_counted() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(
            folder.path(),
            "shop.sqlite",
            &[
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, \"a \"\"b\" REAL)",
                "CREATE TABLE \"line items\" (customer PRIMARY KEY, price) WITHOUT ROWID",
            ],
        );
        let connection = Connection::open(&file, Access::Writes, PATIENT).expect("open");
        connection
            .run("INSERT INTO customers VALUES (1, 'Ada Lovelace of London', 1.0), (5, NULL, 2.5)")
            .unwrap();
        connection
            .run("INSERT INTO \"line items\" VALUES ('b', 2), ('a', 1)")
            .unwrap();
        connection
            .run("CREATE INDEX by_name ON customers (name)")
            .unwrap();
        drop(connection);
        let sheets = sheets(table(&file));
        let names: Vec<&str> = sheets.iter().map(|sheet| sheet.name.as_str()).collect();
        assert_eq!(names, ["customers", "line items"]);
        let customers = &sheets[0];
        assert_eq!(customers.column_names, ["id", "name", "a \"b"]);
        assert_eq!((customers.row_count, customers.rows.len()), (2, 0));
        // "Ada Lovelace of London" is 22 characters at 7 pt plus 17 pt padding; the others are
        // at the minimum width.
        assert_eq!(measured_widths(&file, 0), [56.0, 171.0, 56.0]);
        assert!(customers.column_widths[1] > customers.column_widths[0]);
        assert_eq!(
            texts(&rows(&file, 0, 0..10)),
            [["1", "Ada Lovelace of London", "1.0"], ["5", "", "2.5"]]
        );
        // A table without a row id is read in key order.
        assert_eq!(texts(&rows(&file, 1, 0..10)), [["a", "1"], ["b", "2"]]);
        assert_eq!(texts(&rows(&file, 1, 1..2)), [["b", "2"]]);
        assert!(rows(&file, 7, 0..10).is_empty());
    }

    #[test]
    fn a_column_is_as_wide_as_the_table_s_last_rows_need_too() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(
            folder.path(),
            "grow.sqlite",
            &[
                "CREATE TABLE n (i, label)",
                "WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < 1000) \
                 INSERT INTO n SELECT i * 1000000, 'row' FROM c",
            ],
        );
        // The last row's 1000000000 is 10 characters at 7 pt plus 17 pt padding.
        assert_eq!(measured_widths(&file, 0), [87.0, 56.0]);
    }

    #[test]
    fn values_show_as_what_they_are() {
        let folder = tempfile::tempdir().expect("folder");
        let long = "é".repeat(500);
        let file = made(
            folder.path(),
            "v.sqlite",
            &[
                "CREATE TABLE v (a, b, c, d, e, f)",
                &format!(
                    "INSERT INTO v VALUES (-3, 1e20, '{long}', x'0001ff', \
                     x'000102030405060708090a0b0c0d0e0f10', '')"
                ),
                "INSERT INTO v VALUES (2.0, 0.000001, 'short', NULL, x'', 1.25)",
            ],
        );
        let read = rows(&file, 0, 0..2);
        let shown = texts(&read);
        assert_eq!(shown[0][0], "-3");
        assert_eq!(shown[0][1], "1e20");
        assert_eq!(shown[0][2], "é".repeat(400) + "…");
        assert_eq!(shown[0][3], "x'0001ff'");
        assert_eq!(shown[0][4], "x'000102030405060708090a0b0c0d0e0f…'");
        assert_eq!(read[0][5], None);
        assert_eq!(shown[1], ["2.0", "1e-6", "short", "", "", "1.25"]);
        let alignments: Vec<Option<Alignment>> = read[0]
            .iter()
            .map(|cell| cell.as_ref().map(|cell| cell.alignment))
            .collect();
        assert_eq!(
            alignments,
            [
                Some(Alignment::End),
                Some(Alignment::End),
                Some(Alignment::Start),
                Some(Alignment::Start),
                Some(Alignment::Start),
                None
            ]
        );
    }

    #[test]
    fn rows_deep_in_a_table_are_read_by_row_id_and_gaps_in_the_ids_do_not_move_them() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(
            folder.path(),
            "many.sqlite",
            &[
                "CREATE TABLE n (i)",
                "WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM c WHERE i < 5000) \
                 INSERT INTO n SELECT i FROM c",
                // Delete every third row: row 0 is 1, row 1 is 2, row 2 is 4, row 3 is 5, …
                "DELETE FROM n WHERE i % 3 = 0",
            ],
        );
        let value = |index: usize| (index / 2 * 3 + index % 2 + 1).to_string();
        let mut reading = Reading::of(&file);
        assert_eq!(sheets(reading.table.clone())[0].row_count, 3334);
        for window in [
            0..3,
            1500..1503,
            1400..1402,
            3331..3340,
            900..903,
            1700..1702,
        ] {
            let expected: Vec<Vec<String>> = (window.start..window.end.min(3334))
                .map(|index| vec![value(index)])
                .collect();
            assert_eq!(
                texts(&reading.rows(0, window.clone())),
                expected,
                "{window:?}"
            );
        }
        // Rows prefetched around the last window serve the next one without a file read.
        let held = reading.window.clone().expect("a window");
        assert_eq!((held.1, held.2.len()), (1500, 402));
        assert_eq!(
            texts(&reading.rows(0, 1899..1902)),
            [[value(1899)], [value(1900)], [value(1901)]]
        );
        assert_eq!(reading.window.as_ref().map(|window| window.1), Some(1500));
        assert!(reading.rows(0, 3334..3400).is_empty());
    }

    #[test]
    fn the_view_follows_the_file_and_says_when_it_is_no_database() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "live.sqlite", &["CREATE TABLE t (i)"]);
        assert_eq!(sheets(table(&file))[0].row_count, 0);
        let connection = Connection::open(&file, Access::Writes, PATIENT).expect("open");
        connection.run("PRAGMA journal_mode = WAL").unwrap();
        connection.run("INSERT INTO t VALUES (1), (2)").unwrap();
        // Still open, so the rows are in the write-ahead log.
        assert!(beside(&file, "-wal").exists());
        assert_eq!(sheets(table(&file))[0].row_count, 2);
        assert_eq!(texts(&rows(&file, 0, 0..5)), [["1"], ["2"]]);
        connection.run("INSERT INTO t VALUES (3)").unwrap();
        assert_eq!(sheets(table(&file))[0].row_count, 3);
        drop(connection);
        assert_eq!(texts(&rows(&file, 0, 0..5)), [["1"], ["2"], ["3"]]);
        let empty = folder.path().join("empty.sqlite");
        std::fs::write(&empty, "").unwrap();
        assert!(sheets(table(&empty)).is_empty());
        let words = folder.path().join("words.sqlite");
        std::fs::write(
            &words,
            "no database at all, only a good many words in a file",
        )
        .unwrap();
        assert_eq!(table(&words), Table::Unreadable(Held::Database));
    }

    #[test]
    fn a_card_shows_the_top_of_the_first_table() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(
            folder.path(),
            "c.sqlite",
            &[
                "CREATE TABLE people (name, age)",
                "INSERT INTO people VALUES ('Ada', 36), ('Grace', NULL)",
                "CREATE TABLE other (x)",
            ],
        );
        assert_eq!(card_text(&file), "name   age\nAda    36\nGrace");
        made(
            folder.path(),
            "long.sqlite",
            &[
                "CREATE TABLE t (note, n)",
                "INSERT INTO t VALUES ('a line\nand a long one after it', 1)",
            ],
        );
        assert_eq!(
            card_text(&folder.path().join("long.sqlite")),
            "note                n\na line and a long…  1"
        );
        let none = made(folder.path(), "none.sqlite", &[]);
        assert_eq!(card_text(&none), "");
    }

    fn call(
        file: &Path,
        run: serde_json::Value,
        access: Access,
    ) -> Result<serde_json::Value, String> {
        let statements = asked(&run)?;
        ask(file, &statements, access)
            .map(|answer| serde_json::from_str(&answer).expect("the answer is JSON"))
    }

    #[test]
    fn a_page_reads_and_writes_with_values_and_gets_rows_by_their_columns() {
        let folder = tempfile::tempdir().expect("folder");
        let file = folder.path().join("app.sqlite");
        // A read before any write does not create the file.
        assert_eq!(
            call(
                &file,
                serde_json::json!([["SELECT * FROM notes", []]]),
                Access::Creates
            ),
            Err("no such table: notes".to_owned())
        );
        assert!(!file.exists());
        let made = call(
            &file,
            serde_json::json!([[
                "CREATE TABLE notes (id INTEGER PRIMARY KEY, body, done)",
                []
            ]]),
            Access::Creates,
        )
        .expect("create");
        assert!(is_database(&file));
        assert_eq!(
            made,
            serde_json::json!({"results": [{"columns": [], "rows": [], "changes": 0, "lastInsertRowid": 0}]})
        );
        let put = call(
            &file,
            serde_json::json!([[
                "INSERT INTO notes (body, done) VALUES (?, ?), (?, ?)",
                ["a \"q\"", true, null, 1.5]
            ]]),
            Access::Writes,
        )
        .expect("insert");
        assert_eq!(
            (
                &put["results"][0]["changes"],
                &put["results"][0]["lastInsertRowid"]
            ),
            (&serde_json::json!(2), &serde_json::json!(2))
        );
        let read = call(
            &file,
            serde_json::json!([[
                "SELECT done, body, id, x'00ff' AS raw FROM notes ORDER BY id",
                []
            ]]),
            Access::Reads,
        )
        .expect("select");
        assert_eq!(
            read["results"][0],
            serde_json::json!({
                "columns": ["done", "body", "id", "raw"],
                "rows": [
                    {"done": 1, "body": "a \"q\"", "id": 1, "raw": {"base64": "AP8="}},
                    {"done": 1.5, "body": null, "id": 2, "raw": {"base64": "AP8="}},
                ],
                "changes": 0,
                "lastInsertRowid": 0,
            })
        );
        // Row values follow column order, regardless of how a JSON parser orders keys.
        let text = ask(
            &file,
            &asked(&serde_json::json!([[
                "SELECT done, body FROM notes LIMIT 1",
                []
            ]]))
            .unwrap(),
            Access::Reads,
        )
        .unwrap();
        assert!(text.contains(r#"{"done":1,"body":"a \"q\""}"#), "{text}");
    }

    #[test]
    fn several_statements_happen_together_or_not_at_all() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "t.sqlite", &["CREATE TABLE t (i UNIQUE)"]);
        let both = call(
            &file,
            serde_json::json!([
                ["INSERT INTO t VALUES (1)", []],
                ["INSERT INTO t VALUES (2)", []]
            ]),
            Access::Writes,
        )
        .expect("both");
        assert_eq!(both["results"].as_array().map(Vec::len), Some(2));
        let failed = call(
            &file,
            serde_json::json!([
                ["INSERT INTO t VALUES (3)", []],
                ["INSERT INTO t VALUES (1)", []]
            ]),
            Access::Writes,
        );
        assert_eq!(failed, Err("UNIQUE constraint failed: t.i".to_owned()));
        let read = call(
            &file,
            serde_json::json!([["SELECT count(*) AS n FROM t", []]]),
            Access::Reads,
        )
        .expect("count");
        assert_eq!(read["results"][0]["rows"][0]["n"], 2);
    }

    #[test]
    fn a_reader_is_refused_a_change_and_a_call_says_what_is_wrong_with_it() {
        let folder = tempfile::tempdir().expect("folder");
        let file = made(folder.path(), "t.sqlite", &["CREATE TABLE t (i)"]);
        assert_eq!(
            call(
                &file,
                serde_json::json!([["INSERT INTO t VALUES (1)", []]]),
                Access::Reads
            ),
            Err("this page reads the database and may not change it".to_owned())
        );
        assert_eq!(
            call(
                &file,
                serde_json::json!([["SELECT * FROM nope", []]]),
                Access::Reads
            ),
            Err("no such table: nope".to_owned())
        );
        for malformed in [
            serde_json::json!("SELECT 1"),
            serde_json::json!([["SELECT 1"]]),
            serde_json::json!([[1, []]]),
            serde_json::json!([["SELECT 1", "x"]]),
        ] {
            assert_eq!(
                asked(&malformed),
                Err("the call is not [[sql, [values]], …]".to_owned())
            );
        }
        assert_eq!(
            asked(&serde_json::json!([["SELECT ?", [[1]]]])),
            Err("a value is a number, a text, true, false or null".to_owned())
        );
        assert!(call(
            &folder.path().join("missing.sqlite"),
            serde_json::json!([["SELECT 1", []]]),
            Access::Writes
        )
        .is_err());
    }
}
