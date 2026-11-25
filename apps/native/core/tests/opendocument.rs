//! Checks the OpenDocument spreadsheet reader (`opendocument.rs`) against a file another
//! program wrote: `fixtures/opendocument/Subscription model.ods`, made by odfpy
//! (`fixtures/opendocument/make.py`).

use northroom_core::sheets::spreadsheet::{Alignment, Cell, Sheet, COLUMN_MIN_WIDTH};
use northroom_core::sheets::workbook;
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/opendocument")
        .join(name)
}

fn texts(sheet: &Sheet) -> Vec<Vec<&str>> {
    sheet
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| cell.as_ref().map_or("", |cell| cell.text.as_str()))
                .collect()
        })
        .collect()
}

fn cell(sheet: &Sheet, row: usize, column: usize) -> &Cell {
    sheet.rows[row][column].as_ref().expect("a cell")
}

#[test]
fn a_spreadsheet_odfpy_wrote_reads_as_its_visible_sheets() {
    let workbook = workbook::read(&fixture("Subscription model.ods")).expect("read");
    let names: Vec<&str> = workbook
        .sheets
        .iter()
        .map(|sheet| sheet.name.as_str())
        .collect();
    assert_eq!(names, ["Plans", "First year"]);

    let plans = &workbook.sheets[0];
    assert_eq!(
        texts(plans),
        [
            vec!["Plan", "Bags a month", "Price", "Margin"],
            vec!["Solo", "1", "$19.00", "42%"],
            vec!["Pair", "2", "$33.00", "47%"],
            vec!["Household", "4", "$61.00", "51%"],
            vec![],
            vec![],
            vec!["Roasted in Portland,  Maine", "", "", "15 Jan 2027"],
        ]
    );
    // The first heading names a style of its own, centred and bold through its parent in
    // `styles.xml`; the others take their column's bold style.
    assert_eq!(
        (cell(plans, 0, 0).bold, cell(plans, 0, 0).alignment),
        (true, Alignment::Center)
    );
    assert_eq!(
        (cell(plans, 0, 2).bold, cell(plans, 0, 2).alignment),
        (true, Alignment::Start)
    );
    assert!(!cell(plans, 1, 0).bold);
    assert_eq!(cell(plans, 1, 2).alignment, Alignment::End);
    assert_eq!(cell(plans, 6, 3).alignment, Alignment::End);
    // The first column is declared 6 cm wide, about 227 points, and its longest text needs
    // more; the second declares nothing and fits "Bags a month".
    assert!(plans.column_widths[0] > 6.0 / 2.54 * 96.0);
    assert!(plans.column_widths[1] > COLUMN_MIN_WIDTH);
    assert!(plans.column_widths[1] < plans.column_widths[0]);

    let year = &workbook.sheets[1];
    assert_eq!((year.row_count, year.column_count), (13, 2));
    assert_eq!(texts(year)[12], ["12", "390"]);
}

#[test]
fn a_zip_that_is_not_a_spreadsheet_is_refused() {
    let folder = tempfile::tempdir().expect("a folder");
    let path = folder.path().join("empty.ods");
    std::fs::write(&path, b"not a zip").expect("write");
    assert!(workbook::read(&path).is_err());
}
