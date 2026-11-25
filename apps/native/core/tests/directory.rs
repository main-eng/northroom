//! Checks contacts and calendar files (`sheets::directory`) read as tables, on the sample files
//! the sample-files script writes.

use northroom_core::sheets::spreadsheet::Sheet;
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
fn the_sample_contact_and_calendar_read_as_tables() {
    let samples = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/directory");
    let contact = workbook::read(&samples.join("Maya Ellis.vcf")).expect("read");
    let sheet = &contact.sheets[0];
    assert_eq!(sheet.column_names[..3], ["Name", "Organisation", "Title"]);
    assert_eq!(
        texts(sheet)[0][..2],
        ["Maya Ellis", "Harbor Coffee Roasters"]
    );
    let calendar = workbook::read(&samples.join("Launch calendar.ics")).expect("read");
    let sheet = &calendar.sheets[0];
    assert_eq!(
        texts(sheet)[0][..4],
        [
            "First Harbor Monthly boxes ship",
            "2026-04-01 13:00 UTC",
            "2026-04-01 16:00 UTC",
            "Roastery, 48 Commercial Street, Portland"
        ]
    );
}
