//! The person's settings: one JSON file that the app, the person and their agent all edit
//! (ADR-0014). Its keys are the choices a person makes; what the app remembers on its own (each
//! folder's view, window sizes) stays out of it.
//!
//! ```json
//! {
//!   "drive": "/Users/ada/Northroom",
//!   "appearance": { "theme": "system", "light": { … }, "dark": { … } }
//! }
//! ```
//!
//! Each part of the app owns one key and writes only that key, so a key the app does not know
//! is kept as it was written. A file that is not valid JSON is never written over: the person
//! or the agent who broke it fixes it, and until then the app keeps the settings it last read.

use serde_json::{Map, Value};
use std::path::Path;

/// The file's name, in its folder (`paths::Paths::settings_file`).
pub const NAME: &str = "settings.json";

/// The file's keys, in the order the app writes them.
pub const DRIVE: &str = "drive";
pub const APPEARANCE: &str = "appearance";

/// The settings in the file at `path`, empty if it is missing. Errors with a message for
/// whoever edits it when the file is not a JSON object.
pub fn read(path: &Path) -> Result<Map<String, Value>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(settings)) => Ok(settings),
        Ok(_) => Err(format!("{} is not a JSON object", path.display())),
        Err(error) => Err(format!(
            "{} is not valid JSON (line {}, column {}): {error}",
            path.display(),
            error.line(),
            error.column()
        )),
    }
}

/// Sets `key` to `value` in the file at `path`, leaving every other key alone. Writes a sibling
/// file and renames it over, so the file is never seen half written. Fails when the file exists
/// but does not parse as settings.
pub fn write(path: &Path, key: &str, value: Value) -> std::io::Result<()> {
    let mut settings = read(path).map_err(std::io::Error::other)?;
    settings.insert(key.to_owned(), value);
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut text = serde_json::to_string_pretty(&Value::Object(settings))?;
    text.push('\n');
    let staged = path.with_extension("json.writing");
    std::fs::write(&staged, text)?;
    std::fs::rename(staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn no_file_is_no_settings_and_a_key_is_written_beside_the_others() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join("northroom").join(NAME);
        assert_eq!(read(&path), Ok(Map::new()));
        write(&path, DRIVE, json!("/a")).expect("drive");
        std::fs::write(&path, "{ \"drive\": \"/a\", \"mine\": [1, 2] }\n")
            .expect("an edit by hand");
        write(&path, APPEARANCE, json!({ "theme": "dark" })).expect("appearance");
        let settings = read(&path).expect("settings");
        assert_eq!(settings[DRIVE], json!("/a"));
        assert_eq!(
            settings["mine"],
            json!([1, 2]),
            "a key the app does not know is kept"
        );
        assert_eq!(settings[APPEARANCE], json!({ "theme": "dark" }));
        assert!(std::fs::read_to_string(&path)
            .expect("text")
            .ends_with("}\n"));
        assert!(!path.with_extension("json.writing").exists());
    }

    #[test]
    fn a_broken_file_says_where_and_is_never_written_over() {
        let folder = tempfile::tempdir().expect("folder");
        let path = folder.path().join(NAME);
        std::fs::write(&path, "{\n  \"drive\": \"/a\",\n}\n").expect("broken");
        let why = read(&path).expect_err("broken");
        assert!(why.contains("line 3"), "{why}");
        assert!(write(&path, DRIVE, json!("/b")).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("text"),
            "{\n  \"drive\": \"/a\",\n}\n"
        );
        std::fs::write(&path, "[]").expect("an array");
        assert!(read(&path)
            .expect_err("not an object")
            .contains("not a JSON object"));
        std::fs::write(&path, "  \n").expect("blank");
        assert_eq!(read(&path), Ok(Map::new()));
    }
}
