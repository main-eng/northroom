//! Reads contacts (vCard, `.vcf`) and calendar events (iCalendar, `.ics`) as a table: a row
//! for each contact or event, a column for each field any of them has. Both formats are lines of
//! `NAME;PARAMETERS:value`, a line that starts with a space or tab continuing the one before.
//!
//! Left out: photos and other binary values, alarms (they belong to events but are not shown),
//! recurrence (an event that repeats shows once, at its first time) and time zones (a time is
//! shown as written, with `UTC` when it is in UTC).

use crate::sheets::spreadsheet::{self, Alignment, Cell, Sheet, Workbook};

/// Whether a file name is a contacts or calendar file.
pub fn is_directory_name(extension: &str) -> bool {
    matches!(extension, "vcf" | "vcard" | "ics" | "ical")
}

/// One content line: its name in capitals and its value as written.
struct Line {
    name: String,
    value: String,
}

/// The lines of `text`, with continued lines joined.
fn unfold(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        match (raw.strip_prefix([' ', '\t']), lines.last_mut()) {
            (Some(rest), Some(last)) => last.push_str(rest),
            _ => lines.push(raw.to_owned()),
        }
    }
    lines
}

/// A value with `\n`, `\,`, `\;` and `\\` undone.
fn unescape(value: &str) -> String {
    let mut plain = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            plain.push(character);
            continue;
        }
        match characters.next() {
            Some('n' | 'N') => plain.push('\n'),
            Some(other) => plain.push(other),
            None => {}
        }
    }
    plain
}

fn parse_line(line: &str) -> Option<Line> {
    let colon = line.find(':')?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let name = head.split(';').next().unwrap_or(head);
    // A grouped name (`item1.EMAIL`) is the name after its group.
    let name = name.rsplit('.').next().unwrap_or(name);
    Some(Line {
        name: name.to_ascii_uppercase(),
        value: value.to_owned(),
    })
}

/// The components of `kind` (`VCARD`, `VEVENT`) in the text, each as its lines. Lines of a
/// component nested in one (an alarm in an event) are left out.
fn components(text: &str, kind: &str) -> Vec<Vec<Line>> {
    let mut found = Vec::new();
    let mut current: Option<Vec<Line>> = None;
    let mut depth = 0usize;
    for line in unfold(text).iter().filter_map(|line| parse_line(line)) {
        let value = line.value.trim().to_ascii_uppercase();
        match (line.name.as_str(), current.as_mut()) {
            ("BEGIN", None) if value == kind => current = Some(Vec::new()),
            ("BEGIN", Some(_)) => depth += 1,
            ("END", Some(_)) if depth > 0 => depth -= 1,
            ("END", Some(_)) => found.extend(current.take()),
            (_, Some(lines)) if depth == 0 => lines.push(line),
            _ => {}
        }
    }
    found
}

/// A field shown as a column: its heading and how its text is made from a component's lines.
struct Field {
    heading: &'static str,
    read: fn(&[Line]) -> Option<String>,
    alignment: Alignment,
}

fn first(lines: &[Line], name: &str) -> Option<String> {
    lines
        .iter()
        .find(|line| line.name == name)
        .map(|line| unescape(&line.value))
        .filter(|value| !value.trim().is_empty())
}

/// Every line named `name`, one a line.
fn every(lines: &[Line], name: &str) -> Option<String> {
    let values: Vec<String> = lines
        .iter()
        .filter(|line| line.name == name)
        .map(|line| unescape(&line.value))
        .filter(|value| !value.trim().is_empty())
        .collect();
    (!values.is_empty()).then(|| values.join("\n"))
}

/// The parts of a structured value, split at unescaped `;`.
fn parts(value: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    let mut escaped = false;
    for character in value.chars() {
        match (escaped, character) {
            (false, '\\') => escaped = true,
            (false, ';') => found.push(String::new()),
            _ => {
                escaped = false;
                found.last_mut().expect("a part").push(character);
            }
        }
    }
    found.into_iter().map(|part| unescape(&part)).collect()
}

fn contact_name(lines: &[Line]) -> Option<String> {
    first(lines, "FN").or_else(|| {
        let name = lines.iter().find(|line| line.name == "N")?;
        let parts = parts(&name.value);
        // Family; given; additional; prefix; suffix.
        let order = [3, 1, 2, 0, 4];
        let shown: Vec<&str> = order
            .iter()
            .filter_map(|&at| parts.get(at).map(|part| part.trim()))
            .filter(|part| !part.is_empty())
            .collect();
        (!shown.is_empty()).then(|| shown.join(" "))
    })
}

fn address(lines: &[Line]) -> Option<String> {
    let written: Vec<String> = lines
        .iter()
        .filter(|line| line.name == "ADR")
        .map(|line| {
            let kept: Vec<String> = parts(&line.value)
                .into_iter()
                .map(|part| part.trim().to_owned())
                .filter(|part| !part.is_empty())
                .collect();
            kept.join(", ")
        })
        .filter(|address| !address.is_empty())
        .collect();
    (!written.is_empty()).then(|| written.join("\n"))
}

fn organisation(lines: &[Line]) -> Option<String> {
    let organisation = first(lines, "ORG")?;
    let kept: Vec<String> = parts(&organisation)
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect();
    Some(kept.join(", "))
}

/// A date or date-time as written in a calendar (`20260401`, `20260401T130000Z`), shown as
/// `2026-04-01` or `2026-04-01 13:00 UTC`; anything else as written.
fn when(value: &str) -> String {
    let value = value.trim();
    let digits = |range: std::ops::Range<usize>| {
        value
            .get(range)
            .filter(|part| part.bytes().all(|b| b.is_ascii_digit()))
    };
    let (Some(year), Some(month), Some(day)) = (digits(0..4), digits(4..6), digits(6..8)) else {
        return value.to_owned();
    };
    let date = format!("{year}-{month}-{day}");
    if value.len() == 8 {
        return date;
    }
    let (Some(hour), Some(minute)) = (digits(9..11), digits(11..13)) else {
        return value.to_owned();
    };
    let zone = if value.ends_with('Z') { " UTC" } else { "" };
    format!("{date} {hour}:{minute}{zone}")
}

fn time_of(lines: &[Line], name: &str) -> Option<String> {
    lines
        .iter()
        .find(|line| line.name == name)
        .map(|line| when(&line.value))
}

const CONTACT_FIELDS: [Field; 9] = [
    Field {
        heading: "Name",
        read: contact_name,
        alignment: Alignment::Start,
    },
    Field {
        heading: "Organisation",
        read: organisation,
        alignment: Alignment::Start,
    },
    Field {
        heading: "Title",
        read: |lines| first(lines, "TITLE"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Email",
        read: |lines| every(lines, "EMAIL"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Phone",
        read: |lines| every(lines, "TEL"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Address",
        read: address,
        alignment: Alignment::Start,
    },
    Field {
        heading: "Website",
        read: |lines| every(lines, "URL"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Birthday",
        read: |lines| time_of(lines, "BDAY"),
        alignment: Alignment::End,
    },
    Field {
        heading: "Note",
        read: |lines| first(lines, "NOTE"),
        alignment: Alignment::Start,
    },
];

const EVENT_FIELDS: [Field; 6] = [
    Field {
        heading: "Event",
        read: |lines| first(lines, "SUMMARY"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Starts",
        read: |lines| time_of(lines, "DTSTART"),
        alignment: Alignment::End,
    },
    Field {
        heading: "Ends",
        read: |lines| time_of(lines, "DTEND"),
        alignment: Alignment::End,
    },
    Field {
        heading: "Location",
        read: |lines| first(lines, "LOCATION"),
        alignment: Alignment::Start,
    },
    Field {
        heading: "Organizer",
        read: |lines| {
            first(lines, "ORGANIZER").map(|who| who.trim_start_matches("mailto:").to_owned())
        },
        alignment: Alignment::Start,
    },
    Field {
        heading: "Description",
        read: |lines| first(lines, "DESCRIPTION"),
        alignment: Alignment::Start,
    },
];

/// The components as a sheet named `name`, with a column for each field any of them has.
fn sheet_of(name: &str, found: &[Vec<Line>], fields: &[Field]) -> Sheet {
    let table: Vec<Vec<Option<String>>> = found
        .iter()
        .map(|lines| fields.iter().map(|field| (field.read)(lines)).collect())
        .collect();
    let shown: Vec<usize> = (0..fields.len())
        .filter(|&column| table.iter().any(|row| row[column].is_some()))
        .collect();
    let rows: Vec<Vec<Option<Cell>>> = table
        .into_iter()
        .map(|row| {
            shown
                .iter()
                .map(|&column| {
                    row[column].clone().map(|text| Cell {
                        text,
                        alignment: fields[column].alignment,
                        bold: false,
                    })
                })
                .collect()
        })
        .collect();
    let widths = spreadsheet::measure_column_widths(&rows, shown.len());
    let mut sheet = spreadsheet::sheet(name, rows, shown.len(), widths);
    sheet.column_names = shown
        .iter()
        .map(|&column| fields[column].heading.to_owned())
        .collect();
    sheet
}

/// Reads contacts or events by the file's extension (`vcf`, `ics`) from its text.
pub fn read(extension: &str, text: &str) -> Workbook {
    let sheet = if matches!(extension, "ics" | "ical") {
        sheet_of("Events", &components(text, "VEVENT"), &EVENT_FIELDS)
    } else {
        sheet_of("Contacts", &components(text, "VCARD"), &CONTACT_FIELDS)
    };
    Workbook {
        sheets: vec![sheet],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn contacts_are_rows_with_a_column_for_each_field_any_has() {
        let text = "BEGIN:VCARD\r\nVERSION:3.0\r\nN:Ellis;Maya;;;\r\nFN:Maya Ellis\r\nORG:Harbor Coffee;Roastery\r\nEMAIL;TYPE=WORK:hello@harbor\r\n .example\r\nitem1.TEL:+1 207\r\nADR:;;48 Commercial St;Portland;ME;04101;\r\nPHOTO;ENCODING=b:AAAA\r\nEND:VCARD\r\nBEGIN:VCARD\r\nN:Laurent;Jess;;;\r\nNOTE:Likes\\, a lot\\nof beans\r\nEND:VCARD\r\n";
        let workbook = read("vcf", text);
        let sheet = &workbook.sheets[0];
        assert_eq!(
            sheet.column_names,
            ["Name", "Organisation", "Email", "Phone", "Address", "Note"]
        );
        assert_eq!(
            texts(sheet),
            [
                vec![
                    "Maya Ellis",
                    "Harbor Coffee, Roastery",
                    "hello@harbor.example",
                    "+1 207",
                    "48 Commercial St, Portland, ME, 04101",
                    ""
                ],
                vec!["Jess Laurent", "", "", "", "", "Likes, a lot\nof beans"]
            ]
        );
    }

    #[test]
    fn events_show_their_times_and_leave_their_alarms_out() {
        let text = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20260401T130000Z\nDTEND;VALUE=DATE:20260402\nSUMMARY:Boxes ship\nBEGIN:VALARM\nDESCRIPTION:Alarm\nEND:VALARM\nDESCRIPTION:Real\nEND:VEVENT\nEND:VCALENDAR\n";
        let sheet = &read("ics", text).sheets[0];
        assert_eq!(
            sheet.column_names,
            ["Event", "Starts", "Ends", "Description"]
        );
        assert_eq!(
            texts(sheet),
            [["Boxes ship", "2026-04-01 13:00 UTC", "2026-04-02", "Real"]]
        );
        assert_eq!(when("not a date"), "not a date");
    }
}
