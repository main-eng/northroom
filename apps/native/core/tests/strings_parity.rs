//! Checks that the catalogs (`strings/*.json`) hold exactly the keys native code uses, and that
//! the formatter matches recorded output.
//!
//! The fixture records compiled Paraglide output for every English message native code uses,
//! with plain values and with values a careless formatter would mishandle. It is the reference
//! and does not change with the catalogs. It is English only, the app's one language
//! (ADR-0001).

use northroom_core::strings::{self, NAMES};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn native() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn files_under(directory: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory)
        .expect("a source folder")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            files_under(&path, extension, found);
        } else if path.extension().is_some_and(|ending| ending == extension) {
            found.push(path);
        }
    }
}

/// The names following each `marker` in `text`, made of `allowed` characters.
fn names_after(
    text: &str,
    marker: &str,
    allowed: impl Fn(char) -> bool,
    found: &mut BTreeSet<String>,
) {
    for (at, _) in text.match_indices(marker) {
        // `key::` ending a longer path (`strings::key::`) counts; ending another word
        // (`hotkey::`) does not.
        if text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let rest = &text[at + marker.len()..];
        let name: String = rest.chars().take_while(|c| allowed(*c)).collect();
        if !name.is_empty() {
            found.insert(name.to_lowercase());
        }
    }
}

#[test]
fn the_catalogs_hold_the_keys_native_code_names_and_no_others() {
    let mut used = BTreeSet::new();
    let core = native().join("core/src");
    let mut sources = Vec::new();
    files_under(&core, "rs", &mut sources);
    for path in sources {
        // Skip the lookup module and its tests.
        if path == core.join("strings.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a source file");
        names_after(
            &text,
            "key::",
            |c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_',
            &mut used,
        );
    }
    let table: BTreeSet<String> = NAMES.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(
        used, table,
        "a key in strings/*.json that no code names: take it out of every catalog"
    );
}

#[test]
fn formats_every_message_as_recorded() {
    let cases: Value =
        serde_json::from_str(include_str!("fixtures/strings.json")).expect("fixture");
    let cases = cases.as_array().expect("an array of cases");
    for case in cases {
        let message = case["message"].as_str().expect("message");
        let arguments: Vec<(&str, &str)> = case["args"]
            .as_object()
            .expect("args")
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str().expect("a text value")))
            .collect();
        assert_eq!(
            strings::fill(message, &arguments),
            case["expected"].as_str().expect("expected"),
            "{message:?} {arguments:?}"
        );
    }
    assert!(cases.len() > 600, "the whole fixture is read");
}
