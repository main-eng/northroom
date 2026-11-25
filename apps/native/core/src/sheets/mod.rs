//! Files shown as sheets: delimited text, Excel and OpenDocument workbooks, SQLite databases, and
//! contacts and calendar events.

pub mod archive;
pub mod database;
pub mod directory;
pub mod layout;
pub mod opendocument;
pub mod spreadsheet;
pub(crate) mod sqlite;
pub mod workbook;
pub mod xls;
