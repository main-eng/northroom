//! The part of the sessions menu for the computers this one paired with (`remote::reach`):
//! under each computer's name, the entry that starts a terminal there and the sessions running
//! there. Choosing one opens it in a terminal here.

use super::{Entry, Menu};
use crate::glyph::Glyph;
use crate::remote::reach::{Seen, Standing};
use crate::strings::{self, key};

/// The id prefix of the entry that starts a terminal on a computer, before the computer's key.
const START_ENTRY: &str = "far-start:";
/// The id prefix of a session's row, before the computer's key, a colon and the session.
const SESSION_ENTRY: &str = "far-session:";

/// What a chosen entry of this part asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Chosen<'a> {
    /// A new terminal on the computer with this key.
    Start(&'a str),
    /// The session (second) of the computer with this key (first), shown here.
    Session(&'a str, &'a str),
}

/// What entry `id` asks for; `None` when it is not of this part.
pub fn chosen(id: &str) -> Option<Chosen<'_>> {
    if let Some(key) = id.strip_prefix(START_ENTRY) {
        return Some(Chosen::Start(key));
    }
    // A key is hex, so the first colon ends it whatever the session is called.
    let (key, session) = id.strip_prefix(SESSION_ENTRY)?.split_once(':')?;
    Some(Chosen::Session(key, session))
}

/// A computer's heading: its name, and how it stands while it is not connected.
fn heading(computer: &Seen, language: &str) -> Entry {
    let standing = match computer.standing {
        Standing::Connected => return Entry::label(computer.name.clone()),
        Standing::Connecting => key::COMPUTER_CONNECTING,
        Standing::OutOfReach => key::COMPUTER_OUT_OF_REACH,
    };
    Entry::label(format!(
        "{} · {}",
        computer.name,
        strings::text(language, standing)
    ))
}

/// A computer's rows: its heading, then, while it is connected, the entry that starts a
/// terminal there and its sessions, each with what it is doing under its name.
fn rows(computer: &Seen, language: &str) -> Vec<Entry> {
    let mut rows = vec![heading(computer, language)];
    if computer.standing != Standing::Connected {
        return rows;
    }
    rows.push(
        Entry::item(
            format!("{START_ENTRY}{}", computer.key),
            strings::text(language, key::MENU_NEW_TERMINAL),
        )
        .icon(Glyph::TerminalWindow),
    );
    rows.extend(computer.sessions.iter().map(|session| {
        let id = format!("{SESSION_ENTRY}{}:{}", computer.key, session.id);
        Entry::item(id, session.name.clone())
            .icon(Glyph::Monitor)
            .detail(session.doing.clone())
    }));
    rows
}

/// Adds the paired `computers` to the sessions `menu`, after its rows and before the hidden
/// entries that only carry shortcuts.
pub fn add(menu: &mut Menu, computers: &[Seen], language: &str) {
    let at = menu
        .entries
        .iter()
        .rposition(|entry| !entry.hidden)
        .map_or(0, |last| last + 1);
    let rows = computers
        .iter()
        .flat_map(|computer| rows(computer, language));
    menu.entries.splice(at..at, rows);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::reach::FarSession;

    fn computer(standing: Standing) -> Seen {
        Seen {
            key: "ab12".to_owned(),
            name: "Desktop".to_owned(),
            standing,
            sessions: vec![FarSession {
                id: "s:1".to_owned(),
                name: "Build".to_owned(),
                doing: "Running the tests".to_owned(),
                ..Default::default()
            }],
        }
    }

    fn menu_with(computers: &[Seen]) -> Vec<(String, String)> {
        let mut menu = Menu::new(
            "",
            vec![
                Entry::item("mine", "Mine"),
                Entry::item("next-session", "Next").hidden(),
            ],
        );
        add(&mut menu, computers, "en");
        menu.entries
            .into_iter()
            .map(|entry| (entry.id, entry.title))
            .collect()
    }

    #[test]
    fn a_connected_computer_lists_a_new_terminal_and_its_sessions_before_the_hidden_entries() {
        let listed = menu_with(&[computer(Standing::Connected)]);
        let pair = |id: &str, title: &str| (id.to_owned(), title.to_owned());
        assert_eq!(
            listed,
            [
                pair("mine", "Mine"),
                pair("", "Desktop"),
                pair("far-start:ab12", "New terminal"),
                pair("far-session:ab12:s:1", "Build"),
                pair("next-session", "Next"),
            ]
        );
        assert_eq!(chosen("far-start:ab12"), Some(Chosen::Start("ab12")));
        // A session's own name may hold a colon; the key never does.
        assert_eq!(
            chosen("far-session:ab12:s:1"),
            Some(Chosen::Session("ab12", "s:1"))
        );
        assert_eq!(chosen("session:ab12"), None);
    }

    #[test]
    fn a_computer_that_is_not_connected_says_so_and_lists_nothing_to_choose() {
        let listed = menu_with(&[computer(Standing::OutOfReach)]);
        assert_eq!(
            listed[1],
            (String::new(), "Desktop · Out of reach".to_owned())
        );
        assert_eq!(listed.len(), 3);
        let connecting = menu_with(&[computer(Standing::Connecting)]);
        assert_eq!(connecting[1].1, "Desktop · Connecting…");
        assert_eq!(menu_with(&[]).len(), 2);
    }
}
