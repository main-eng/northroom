//! Generates the core's string table from the message catalogs in `strings/<language>.json`,
//! the single source of user-facing text (`src/strings.rs`). To add a key, write it into every
//! catalog and reference it in Rust as `key::NAME`; `tests/strings_parity.rs` fails on a
//! catalog key that no code uses.
//!
//! The app is English only (ADR-0001), so `strings/` holds only `en.json`. To add a language,
//! add its catalog and its code to `LANGUAGES`; the checks below hold it to English's keys and
//! placeholders.
//!
//! The build fails if a catalog is missing a key English has or has one English lacks, if a
//! message uses any format syntax besides `{name}`, or if a translation uses different
//! placeholders from English.
//!
//! Also writes `DRAWING`, a digest of the page-host files used to draw 3D model stills, so a
//! cached still is redrawn only when those files change (`src/model_still.rs`).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[path = "src/digest.rs"]
#[allow(dead_code)]
mod digest;

/// Files and folders, relative to the core, used to draw a 3D model still: the page host's
/// scripts, its pinned libraries, and each platform's web view and capture code.
const DRAWING: [&str; 7] = [
    "assets/host",
    "src/page/host",
    "../gpui/webhost/resources",
    "../gpui/webhost/src/mac/page_host.rs",
    "../gpui/webhost/src/mac/web.rs",
    "../gpui/webhost/src/mac/pixels.rs",
    "../gpui/webhost/src/elsewhere",
];

/// Supported languages, each with a catalog at `strings/<code>.json`. The index is the message
/// column. English comes first, and an unsupported language falls back to it.
const LANGUAGES: [&str; 1] = ["en"];

fn main() {
    // Read at run time, not compile time: one build script serves every checkout that shares a
    // target folder, and each must read its own catalogs.
    let manifest = std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    write_drawing(Path::new(&manifest));
    let catalogs = Path::new(&manifest).join("strings");
    println!("cargo:rerun-if-changed={}", catalogs.display());

    let mut files: Vec<String> = std::fs::read_dir(&catalogs)
        .unwrap_or_else(|error| panic!("{}: {error}", catalogs.display()))
        .map(|entry| {
            entry
                .expect("a catalog entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    files.sort();
    let mut expected: Vec<String> = LANGUAGES
        .iter()
        .map(|code| format!("{code}.json"))
        .collect();
    expected.sort();
    assert_eq!(
        files, expected,
        "strings/ holds one catalog for each language of build.rs's LANGUAGES, and no other"
    );

    let columns: Vec<BTreeMap<String, String>> = LANGUAGES
        .iter()
        .map(|language| {
            let path = catalogs.join(format!("{language}.json"));
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            serde_json::from_str(&text).unwrap_or_else(|error| {
                panic!("{}: not an object of messages: {error}", path.display())
            })
        })
        .collect();

    let english = &columns[0];
    let names: Vec<&String> = english.keys().collect();
    assert!(names.len() <= usize::from(u16::MAX), "a key is a u16");
    for name in &names {
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "en {name}: a key is written in lower case, digits and underscores"
        );
    }
    for (language, catalog) in LANGUAGES.iter().zip(&columns) {
        let extra: Vec<&String> = catalog
            .keys()
            .filter(|name| !english.contains_key(*name))
            .collect();
        assert!(
            extra.is_empty(),
            "{language} has keys English has not: {extra:?}"
        );
        for name in &names {
            let message = catalog.get(*name).unwrap_or_else(|| {
                panic!("{language} has no {name}: write it in strings/{language}.json")
            });
            let taken = placeholders(message).unwrap_or_else(|| {
                panic!("{language} {name} uses a construct the core does not format: {message}")
            });
            assert_eq!(
                taken,
                placeholders(&english[*name]).expect("English formats"),
                "{language} {name} takes other names than English does"
            );
        }
    }

    let mut table =
        String::from("// Written by build.rs from strings/*.json.\n\nuse super::Key;\n\n");
    let quoted: Vec<String> = LANGUAGES.iter().map(|code| format!("{code:?}")).collect();
    writeln!(
        table,
        "/// The languages the app has, English first; a message's column."
    )
    .unwrap();
    writeln!(
        table,
        "pub const LANGUAGES: [&str; {}] = [{}];\n",
        LANGUAGES.len(),
        quoted.join(", ")
    )
    .unwrap();
    writeln!(table, "/// The catalog keys, sorted; a message's row.").unwrap();
    writeln!(table, "pub const NAMES: [&str; {}] = [", names.len()).unwrap();
    for name in &names {
        writeln!(table, "    {name:?},").unwrap();
    }
    writeln!(table, "];\n").unwrap();
    writeln!(table, "/// Every key's message in every language.").unwrap();
    writeln!(
        table,
        "pub static MESSAGES: [[&str; {}]; {}] = [",
        LANGUAGES.len(),
        names.len()
    )
    .unwrap();
    for name in &names {
        let row: Vec<String> = columns
            .iter()
            .map(|catalog| format!("{:?}", catalog[*name]))
            .collect();
        writeln!(table, "    [{}],", row.join(", ")).unwrap();
    }
    writeln!(table, "];\n").unwrap();
    writeln!(table, "/// The keys by name.").unwrap();
    writeln!(table, "pub mod key {{\n    use super::Key;").unwrap();
    for (index, name) in names.iter().enumerate() {
        writeln!(
            table,
            "    pub const {}: Key = Key({index});",
            name.to_uppercase()
        )
        .unwrap();
    }
    writeln!(table, "}}").unwrap();

    let out =
        Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("strings_table.rs");
    std::fs::write(&out, table).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
}

/// Writes `drawing.rs` with `DRAWING`: 32 hex digits digesting the path and bytes of every file
/// `DRAWING` names. Missing files are hashed as missing.
fn write_drawing(manifest: &Path) {
    let mut files = Vec::new();
    for named in DRAWING {
        let path = manifest.join(named);
        println!("cargo:rerun-if-changed={}", path.display());
        list_files(&path, named, &mut files);
    }
    files.sort();
    let mut text = Vec::new();
    for (name, path) in files {
        let bytes = std::fs::read(&path).unwrap_or_default();
        text.extend_from_slice(format!("{name} {}\n", bytes.len()).as_bytes());
        text.extend_from_slice(&bytes);
    }
    let identity = &digest::sha256_hex(&text)[..32];
    let out = Path::new(&std::env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("drawing.rs");
    let code = format!(
        "/// What a 3D model's still is drawn with (`build.rs`).\npub const DRAWING: &str = \"{identity}\";\n"
    );
    std::fs::write(&out, code).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
}

/// Adds each file at or under `path` to `files`, keyed by its path from the core (`named` for
/// `path`). A path that does not exist is added as missing.
fn list_files(path: &Path, named: &str, files: &mut Vec<(String, PathBuf)>) {
    let Ok(metadata) = std::fs::metadata(path) else {
        files.push((format!("{named} missing"), PathBuf::new()));
        return;
    };
    if !metadata.is_dir() {
        files.push((named.to_owned(), path.to_path_buf()));
        return;
    }
    let entries =
        std::fs::read_dir(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    for entry in entries {
        let entry = entry.expect("a folder entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            list_files(&entry.path(), &format!("{named}/{name}"), files);
        }
    }
}

/// Placeholder names in a message (`{word}` gives `word`). `None` if any brace is not a plain
/// placeholder, the only format syntax the core supports.
fn placeholders(message: &str) -> Option<BTreeSet<&str>> {
    let mut names = BTreeSet::new();
    let mut rest = message;
    while let Some(at) = rest.find(['{', '}']) {
        if rest[at..].starts_with('}') {
            return None;
        }
        let after = &rest[at + 1..];
        let close = after.find('}')?;
        let name = &after[..close];
        let is_name = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !is_name {
            return None;
        }
        names.insert(name);
        rest = &after[close + 1..];
    }
    Some(names)
}
