//! A zip archive as a table of what it holds: each file's path, its size unpacked and when it
//! last changed, folders left out as their files' paths name them. Nothing is unpacked.

use crate::sheets::spreadsheet::{self, Alignment, Cell, Sheet, Workbook};
use crate::sheets::workbook::{Result, WorkbookError};

pub fn is_archive_name(extension: &str) -> bool {
    extension == "zip"
}

/// Files a system adds to an archive for itself, not the person's.
fn is_system_entry(name: &str) -> bool {
    name.starts_with("__MACOSX/") || name.ends_with(".DS_Store")
}

fn cell(text: String, alignment: Alignment) -> Option<Cell> {
    Some(Cell {
        text,
        alignment,
        bold: false,
    })
}

/// When an entry last changed, worded as the file list words dates.
fn modified_words(modified: Option<(u16, u8, u8, u8, u8)>) -> String {
    match modified {
        Some((year, month, day, hour, minute)) => crate::list::dated(
            usize::from(month).saturating_sub(1),
            u32::from(day),
            i64::from(year),
            &crate::list::clock_of(u32::from(hour), u32::from(minute)),
        ),
        None => "--".to_owned(),
    }
}

/// The archive at `path`'s files as one table: Name, Size and Date Modified.
pub fn read(path: &std::path::Path) -> Result<Workbook> {
    let listed = crate::package::list_file(path).map_err(|error| WorkbookError(error.0))?;
    let rows: Vec<Vec<Option<Cell>>> = listed
        .into_iter()
        .filter(|entry| !entry.name.ends_with('/') && !is_system_entry(&entry.name))
        .map(|entry| {
            vec![
                cell(entry.name, Alignment::Start),
                cell(
                    crate::list::format_size(Some(entry.size as u64)),
                    Alignment::End,
                ),
                cell(modified_words(entry.modified), Alignment::Start),
            ]
        })
        .collect();
    let widths = spreadsheet::measure_column_widths(&rows, 3);
    let mut sheet: Sheet = spreadsheet::sheet("Contents", rows, 3, widths);
    sheet.column_names = vec!["Name".into(), "Size".into(), "Date Modified".into()];
    Ok(Workbook {
        sheets: vec![sheet],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(sheet: &Sheet) -> Vec<Vec<String>> {
        sheet
            .rows
            .iter()
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

    #[test]
    fn an_archive_lists_its_files_with_their_sizes_and_dates() {
        let root = tempfile::tempdir().expect("a folder");
        let kit = root.path().join("Press kit");
        std::fs::create_dir(&kit).expect("made");
        std::fs::write(kit.join("Logo.svg"), [b'x'; 2400]).expect("written");
        std::fs::write(kit.join("About.txt"), b"Harbor Coffee").expect("written");
        let zip = crate::archive::Compress::of(vec![kit], "en")
            .expect("planned")
            .run()
            .expect("compressed");
        let workbook = read(&zip).expect("listed");
        let sheet = &workbook.sheets[0];
        assert_eq!(sheet.column_names, ["Name", "Size", "Date Modified"]);
        let mut rows = texts(sheet);
        rows.sort();
        assert_eq!(rows.len(), 2, "folders are left out: {rows:?}");
        assert_eq!(
            rows[0][..2],
            ["Press kit/About.txt".to_owned(), "13 bytes".to_owned()]
        );
        assert_eq!(
            rows[1][..2],
            ["Press kit/Logo.svg".to_owned(), "2 KB".to_owned()]
        );
        assert!(rows[0][2].contains(" at "), "{}", rows[0][2]);
    }
}
