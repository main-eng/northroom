//! User-facing text in the interface language. The message catalogs (`strings/<language>.json`)
//! are the source: `build.rs` compiles them into the table here and rejects a catalog the core
//! cannot format. `tests/strings_parity.rs` checks the catalogs against the keys native code
//! uses, and the formatter against recorded output.
//!
//! Every word a person reads comes from here, including menus, dialogs, toasts, placeholders
//! and tooltips. Text only agents or checks read, such as log lines and agent tool output,
//! stays as literals.
//!
//! The app is English only (ADR-0001): `en.json` is the one catalog and the interface language
//! is always English (`ffi::interface_language`). Every caller still passes a `language`, so
//! another language is added as a catalog and nothing else changes here.

mod table {
    include!(concat!(env!("OUT_DIR"), "/strings_table.rs"));
}

pub use table::{key, LANGUAGES, NAMES};

/// A catalog key the native app uses: one of the constants in `key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key(u16);

impl Key {
    /// The key as the catalogs spell it: `account_cancel`.
    pub fn name(self) -> &'static str {
        NAMES[usize::from(self.0)]
    }
}

/// The key the catalogs spell `name`, for a check of the table against the catalogs.
pub fn named(name: &str) -> Option<Key> {
    NAMES
        .binary_search(&name)
        .ok()
        .map(|index| Key(index as u16))
}

/// The raw catalog message in `language`, placeholders included. A code not in `LANGUAGES`
/// falls back to English.
pub fn text(language: &str, key: Key) -> &'static str {
    let column = LANGUAGES
        .iter()
        .position(|known| *known == language)
        .unwrap_or(0);
    table::MESSAGES[usize::from(key.0)][column]
}

/// A message in `language` with each `{name}` replaced by the value `arguments` gives it.
pub fn format(language: &str, key: Key, arguments: &[(&str, &str)]) -> String {
    fill(text(language, key), arguments)
}

/// `message` with each `{name}` replaced by its value from `arguments`. Inserted values are not
/// scanned again, and a name with no value becomes "undefined".
pub fn fill(message: &str, arguments: &[(&str, &str)]) -> String {
    let mut rest = message;
    let mut out = String::with_capacity(rest.len());
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let name_length = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        if name_length > 0 && after[name_length..].starts_with('}') {
            let name = &after[..name_length];
            out.push_str(
                arguments
                    .iter()
                    .find(|(given, _)| *given == name)
                    .map_or("undefined", |(_, value)| value),
            );
            rest = &after[name_length + 1..];
        } else {
            out.push('{');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// A message resolved later in whatever language the reader uses: a key plus its placeholder
/// values. Errors that may reach both people and agents carry one. `Display` renders English.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message {
    pub key: Key,
    pub arguments: Vec<(&'static str, String)>,
}

impl Message {
    pub fn new(key: Key) -> Self {
        Self {
            key,
            arguments: Vec::new(),
        }
    }

    /// The message with `{name}` given `value`.
    pub fn with(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.arguments.push((name, value.into()));
        self
    }

    /// The message in `language`.
    pub fn say(&self, language: &str) -> String {
        let arguments: Vec<(&str, &str)> = self
            .arguments
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        format(language, self.key, &arguments)
    }
}

impl std::fmt::Display for Message {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.say(LANGUAGES[0]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_found_by_name_and_know_their_own() {
        assert!(
            NAMES.windows(2).all(|pair| pair[0] < pair[1]),
            "sorted, for the search"
        );
        for (index, name) in NAMES.iter().enumerate() {
            assert_eq!(named(name), Some(Key(index as u16)));
            assert_eq!(Key(index as u16).name(), *name);
        }
        assert_eq!(named("no_such_key"), None);
        assert_eq!(key::ACCOUNT_CANCEL.name(), "account_cancel");
    }

    #[test]
    fn a_message_reads_in_english_and_so_does_a_language_the_app_has_not() {
        assert_eq!(LANGUAGES, ["en"]);
        assert_eq!(text("en", key::ACCOUNT_CANCEL), "Cancel");
        assert_eq!(text("de", key::ACCOUNT_CANCEL), "Cancel");
    }

    #[test]
    fn names_are_replaced_once_and_only_names() {
        assert_eq!(
            format("en", key::DRIVE_NAME_TAKEN, &[("name", "noon")]),
            "A file named “noon” already exists"
        );
        assert_eq!(
            format("en", key::DRIVE_NAME_TAKEN, &[("name", "{name}")]),
            "A file named “{name}” already exists"
        );
        assert_eq!(
            format("en", key::DRIVE_NAME_TAKEN, &[]),
            "A file named “undefined” already exists"
        );
        assert_eq!(
            format("en", key::ACCOUNT_CANCEL, &[("when", "noon")]),
            "Cancel"
        );
    }

    #[test]
    fn a_message_is_said_with_its_values_and_in_english_by_default() {
        let message = Message::new(key::DRIVE_NAME_TAKEN).with("name", "noon");
        assert_eq!(message.say("en"), "A file named “noon” already exists");
        assert_eq!(message.to_string(), "A file named “noon” already exists");
    }
}
