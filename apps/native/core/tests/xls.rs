//! Checks the Excel 97–2003 reader (`sheets::xls`) against a file xlwt wrote
//! (`fixtures/workbooks/make_xls.py`).

use northroom_core::sheets::spreadsheet::{Alignment, Sheet};
use northroom_core::sheets::workbook;
use std::path::PathBuf;

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

#[test]
fn a_workbook_xlwt_wrote_reads_as_its_visible_sheets() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workbooks/xlwt.xls");
    let workbook = workbook::read(&path).expect("read");
    let names: Vec<&str> = workbook
        .sheets
        .iter()
        .map(|sheet| sheet.name.as_str())
        .collect();
    assert_eq!(names, ["Plans", "Notes"]);
    let plans = &workbook.sheets[0];
    assert_eq!(
        texts(plans),
        [
            vec!["Plan", "Bags", "Price", "Margin"],
            vec!["Solo", "1", "$19.00", "42%"],
            vec!["Pair", "2", "$33.50", "47%"],
            vec!["Launch", "2026-04-01"],
            // xlwt saves no result for its formula, so the cell shows nothing, as Excel would
            // until it calculates.
            vec!["TRUE"],
        ]
    );
    let heading = plans.rows[0][0].as_ref().expect("a cell");
    assert!(heading.bold);
    assert!(!plans.rows[1][0].as_ref().expect("a cell").bold);
    assert_eq!(
        plans.rows[1][2].as_ref().expect("a cell").alignment,
        Alignment::End
    );
    assert_eq!(plans.column_widths[0], 20.0 * 8.0 + 17.0);
    let notes = &workbook.sheets[1];
    assert_eq!(notes.row_count, 900);
    assert_eq!(texts(notes)[0][0].len(), 9005);
    assert_eq!(texts(notes)[899][0], "note 899");
}
