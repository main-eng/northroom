//! Checks the inline editing rules against recorded results (`fixtures/inline_edit.json`) for
//! the same texts, ranges and edits.

use northroom_core::page::inline_edit::{
    delete_range, has_mark_at, is_at_end, is_at_start, join, link_range, replace_range,
    revealed_link_end, slice, snap_offset, split, toggle_mark, tokens, Construct, Token,
};
use serde_json::{json, Value};

fn units(value: &Value) -> Vec<u16> {
    value.as_str().expect("text").encode_utf16().collect()
}

fn text(units: &[u16]) -> Value {
    // These inputs never produce lone surrogates; a lossy decode would show as a mismatch.
    json!(String::from_utf16_lossy(units))
}

fn number(value: &Value) -> usize {
    value.as_u64().expect("number") as usize
}

/// The marks checked at each offset and range.
const MARKS: [Construct; 4] = [
    Construct::Bold,
    Construct::Italic,
    Construct::Strike,
    Construct::Code,
];

/// A token as the fixture writes it.
fn token_json(token: &Token) -> Value {
    match token {
        Token::Text {
            start,
            end,
            marks,
            image,
            ..
        } => {
            json!([
                "text",
                start,
                end,
                marks
                    .iter()
                    .map(|mark| mark.name())
                    .collect::<Vec<_>>()
                    .join(","),
                image
            ])
        }
        Token::Open {
            start,
            end,
            construct,
            kind,
        } => json!(["open", start, end, kind.name(), construct]),
        Token::Close {
            start,
            end,
            construct,
            kind,
        } => json!(["close", start, end, kind.name(), construct]),
    }
}

/// Checks the rules at one offset of `raw` against its fixture entry.
fn check_offset(raw: &[u16], shown: &str, entry: &Value) {
    let offset = number(&entry[0]);
    let pair = |range: Option<(usize, usize)>| {
        range.map_or(
            Value::Null,
            |(start, end)| json!({ "start": start, "end": end }),
        )
    };
    let got = json!([
        offset,
        snap_offset(raw, offset),
        is_at_start(raw, offset),
        is_at_end(raw, offset),
        pair(delete_range(raw, offset, true)),
        pair(delete_range(raw, offset, false)),
        MARKS
            .iter()
            .map(|mark| has_mark_at(raw, offset, *mark))
            .collect::<Vec<_>>(),
        revealed_link_end(raw, offset),
    ]);
    assert_eq!(&got, entry, "offset {offset} of {shown:?}");
}

/// Checks every edit of one range of `raw` (replaced by each typed text, each mark toggled,
/// split, sliced, linked) against its fixture entry. Returns how many were checked.
fn check_range(raw: &[u16], shown: &str, entry: &Value, typed: &[Vec<u16>]) -> usize {
    let range = (number(&entry[0]), number(&entry[1]));
    for (inserted, expected) in typed.iter().zip(entry[2].as_array().expect("replacements")) {
        let edit = replace_range(raw, range, inserted);
        assert_eq!(
            json!([text(&edit.text), edit.caret]),
            *expected,
            "replace {range:?} of {shown:?} with {:?}",
            String::from_utf16_lossy(inserted)
        );
    }
    for (mark, expected) in MARKS.iter().zip(entry[3].as_array().expect("toggles")) {
        let got = toggle_mark(raw, range, *mark).map_or(Value::Null, |(toggled, start, end)| {
            json!([text(&toggled), start, end])
        });
        assert_eq!(
            got,
            *expected,
            "toggle {} over {range:?} of {shown:?}",
            mark.name()
        );
    }
    let (before, after) = split(raw, range);
    assert_eq!(
        json!([text(&before), text(&after)]),
        entry[4],
        "split {range:?} of {shown:?}"
    );
    assert_eq!(
        text(&slice(raw, range)),
        entry[5],
        "slice {range:?} of {shown:?}"
    );
    for (url, expected) in ["", "https://x.y/z"]
        .iter()
        .zip(entry[6].as_array().expect("links"))
    {
        let address: Vec<u16> = url.encode_utf16().collect();
        let edit = link_range(raw, range, &address);
        assert_eq!(
            json!([text(&edit.text), edit.caret]),
            *expected,
            "link {range:?} of {shown:?} to {url:?}"
        );
    }
    12
}

#[test]
fn edits_inline_marks_as_recorded() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/inline_edit.json")).expect("fixture");
    let typed: Vec<Vec<u16>> = fixture["typed"]
        .as_array()
        .expect("typed")
        .iter()
        .map(units)
        .collect();
    let mut checked = 0;

    for case in fixture["cases"].as_array().expect("cases") {
        let raw = units(&case["raw"]);
        let shown = case["raw"].as_str().expect("raw");
        let found: Vec<Value> = tokens(&raw).iter().map(token_json).collect();
        assert_eq!(json!(found), case["tokens"], "tokens of {shown:?}");
        for entry in case["offsets"].as_array().expect("offsets") {
            check_offset(&raw, shown, entry);
            checked += 1;
        }
        for entry in case["ranges"].as_array().expect("ranges") {
            checked += check_range(&raw, shown, entry, &typed);
        }
    }

    for entry in fixture["joins"].as_array().expect("joins") {
        let edit = join(&units(&entry[0]), &units(&entry[1]));
        assert_eq!(
            json!([text(&edit.text), edit.caret]),
            json!([entry[2], entry[3]]),
            "join {:?} + {:?}",
            entry[0],
            entry[1]
        );
        checked += 1;
    }
    assert!(
        checked > 12_000,
        "the fixture is the size it was written at: {checked}"
    );
}
