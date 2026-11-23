//! Checks the workbook reader (`workbook.rs`) against a recorded reading
//! (`fixtures/workbook.json`) of every workbook in `fixtures/workbooks` and of a table of values
//! under format codes.

use northroom_core::sheets::spreadsheet::{Alignment, Workbook};
use northroom_core::sheets::workbook::{self, DateSystem};
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The workbook in the fixture's shape, with numbers as doubles.
fn in_fixture_shape(workbook: &Workbook) -> Value {
    let sheets: Vec<Value> = workbook
        .sheets
        .iter()
        .map(|sheet| {
            json!({
                "name": sheet.name,
                "columnCount": sheet.column_count as f64,
                "columnWidthsPx": sheet.column_widths,
                "rows": sheet.rows.iter().map(|row| row.iter().map(|cell| match cell {
                    Some(cell) => json!({
                        "text": cell.text,
                        "alignment": match cell.alignment { Alignment::Start => "start", Alignment::Center => "center", Alignment::End => "end" },
                        "bold": cell.bold,
                    }),
                    None => Value::Null,
                }).collect::<Vec<_>>()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({ "sheets": sheets })
}

/// Numbers compared as doubles, whatever JSON wrote them as.
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, value)| b.get(key).is_some_and(|other| same(value, other)))
        }
        _ => a == b,
    }
}

/// Panics at the first sheet field or row where workbook `name` differs from the recording,
/// else with both in full.
fn report_difference(name: &str, native: &Value, recorded: &Value) -> ! {
    for (index, (a, b)) in native["sheets"]
        .as_array()
        .unwrap()
        .iter()
        .zip(recorded["sheets"].as_array().unwrap())
        .enumerate()
    {
        for key in ["name", "columnCount", "columnWidthsPx"] {
            assert!(
                same(&a[key], &b[key]),
                "{name} sheet {index} {key}: core {} recorded {}",
                a[key],
                b[key]
            );
        }
        for (row, (x, y)) in a["rows"]
            .as_array()
            .unwrap()
            .iter()
            .zip(b["rows"].as_array().unwrap())
            .enumerate()
        {
            assert!(
                same(x, y),
                "{name} sheet {index} row {}: core {x} recorded {y}",
                row + 1
            );
        }
    }
    panic!("{name}: differs: core {native} recorded {recorded}");
}

#[test]
fn every_workbook_reads_as_recorded() {
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join("workbook.json")).unwrap())
            .unwrap();
    let workbooks = fixture["workbooks"].as_object().unwrap();
    assert!(workbooks.len() >= 20, "{} workbooks", workbooks.len());
    let mut cells = 0;
    for (name, recorded) in workbooks {
        let read = workbook::read(&fixtures().join("workbooks").join(name));
        match (recorded.get("error"), read) {
            (Some(_), Err(_)) => {}
            (Some(_), Ok(workbook)) => {
                panic!("{name}: the recording refuses it, the core read {workbook:?}")
            }
            (None, Err(failure)) => panic!("{name}: the core refused it: {failure}"),
            (None, Ok(workbook)) => {
                let native = in_fixture_shape(&workbook);
                if !same(&native, recorded) {
                    report_difference(name, &native, recorded);
                }
                cells += workbook
                    .sheets
                    .iter()
                    .flat_map(|sheet| &sheet.rows)
                    .flatten()
                    .flatten()
                    .count();
            }
        }
    }
    assert!(cells > 13_000, "{cells} cells compared");
}

#[test]
fn every_number_format_writes_as_recorded() {
    let fixture: Value =
        serde_json::from_str(&std::fs::read_to_string(fixtures().join("workbook.json")).unwrap())
            .unwrap();
    let formats = fixture["formats"].as_array().unwrap();
    assert!(formats.len() > 15_000);
    let mut failed = Vec::new();
    for case in formats {
        // Parse from text, since serde_json's double parsing may be off by one ulp.
        let value: f64 = case["value"].as_str().unwrap().parse().unwrap();
        let code = case["code"].as_str().unwrap();
        let system = if case["system"] == "1904" {
            DateSystem::From1904
        } else {
            DateSystem::From1900
        };
        let native = workbook::format_number(value, code, system).ok();
        let recorded = case["text"].as_str().map(str::to_owned);
        if native != recorded {
            failed.push(format!(
                "{value} under {code:?} ({:?}): core {native:?} recorded {recorded:?}",
                case["system"]
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} differ:\n{}",
        failed.len(),
        formats.len(),
        failed[..failed.len().min(40)].join("\n")
    );
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Inflates every raw deflate stream in the cases archive and checks that each is refused one
/// byte under its size. Returns how many there were.
fn inflate_every_stream(archive: &workbook::ZipArchive) -> usize {
    let names: Vec<String> = archive.entry_names().map(str::to_owned).collect();
    let mut streams = 0;
    for name in names.iter().filter(|name| name.ends_with(".deflate")) {
        let case = name.split('-').next().unwrap();
        let plain = archive
            .read_entry(&format!("{case}.plain"), usize::MAX)
            .unwrap()
            .unwrap();
        let (_, stream) = archive.raw_entry(name).unwrap().unwrap();
        assert_eq!(
            workbook::inflate(stream, usize::MAX).unwrap(),
            plain,
            "{name}"
        );
        // One byte under its size is refused, not truncated.
        if !plain.is_empty() {
            assert!(
                workbook::inflate(stream, plain.len() - 1).is_err(),
                "{name} past its limit"
            );
        }
        streams += 1;
    }
    streams
}

/// The CRC of the entry `name`, from the zip's central directory.
fn recorded_crc(bytes: &[u8], name: &str) -> u32 {
    let directory = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .unwrap();
    let mut at =
        u32::from_le_bytes(bytes[directory + 16..directory + 20].try_into().unwrap()) as usize;
    loop {
        let length = u16::from_le_bytes([bytes[at + 28], bytes[at + 29]]) as usize;
        if bytes[at + 46..at + 46 + length] == *name.as_bytes() {
            return u32::from_le_bytes(bytes[at + 16..at + 20].try_into().unwrap());
        }
        at += 46
            + length
            + u16::from_le_bytes([bytes[at + 30], bytes[at + 31]]) as usize
            + u16::from_le_bytes([bytes[at + 32], bytes[at + 33]]) as usize;
    }
}

/// Checks each readable part of the workbook at `path` against the CRC Python's zipfile wrote.
/// Returns how many matched.
fn check_workbook_parts(path: &std::path::Path) -> usize {
    let bytes = std::fs::read(path).unwrap();
    let Ok(archive) = workbook::ZipArchive::open(&bytes) else {
        return 0;
    };
    let names: Vec<String> = archive.entry_names().map(str::to_owned).collect();
    let mut parts = 0;
    for name in names {
        let Ok(Some(read)) = archive.read_entry(&name, usize::MAX) else {
            continue;
        };
        let crc = recorded_crc(&bytes, &name);
        if path.file_name().unwrap() == "damaged-but-readable.xlsx"
            && name == "xl/worksheets/sheet1.xml"
        {
            assert_ne!(
                crc32(&read),
                crc,
                "the damaged part reads, but not as written"
            );
            continue;
        }
        assert_eq!(crc32(&read), crc, "{} {name}", path.display());
        parts += 1;
    }
    parts
}

/// Inflates every raw deflate stream zlib made at each strategy and level, and every deflated
/// workbook part, checking the latter against Python zipfile's CRCs.
#[test]
fn inflates_what_python_and_zlib_write() {
    let bytes = std::fs::read(fixtures().join("workbooks/inflate-cases.zip")).unwrap();
    let archive = workbook::ZipArchive::open(&bytes).unwrap();
    assert_eq!(inflate_every_stream(&archive), 5 * 11);
    let mut parts = 0;
    for entry in std::fs::read_dir(fixtures().join("workbooks")).unwrap() {
        parts += check_workbook_parts(&entry.unwrap().path());
    }
    assert!(parts > 150, "{parts} parts");
}
