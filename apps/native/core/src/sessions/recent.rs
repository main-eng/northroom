//! The agent conversations last seen in a terminal, kept per drive so one that has ended can be
//! opened again from the sessions menu's "Recent" group. Only a conversation whose id its agent
//! reported is kept, since only that one can be resumed exactly (`terminal::resume`).
//!
//! The list is a JSON file in the drive's app data, most recent first. It is read strictly: a
//! file that does not parse, or holds a value that could not have been written here, counts as
//! empty.

pub mod ffi;

use crate::list::Local;
use crate::strings::{self, key};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// How many conversations are kept.
pub const KEPT: usize = 20;
/// How many the menu lists.
pub const SHOWN: usize = 5;
/// The list's file in the drive's app data.
const FILE: &str = "recent-conversations.json";

/// One conversation as it was last seen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversation {
    /// The id its agent reported (`resume::is_conversation_id`).
    pub id: String,
    /// The agent's label (`Agent::label`).
    pub agent: String,
    /// The command that resumes it, typed into a new terminal's shell (`resume::command`).
    pub command: String,
    /// The folder it ran in, as an absolute path.
    pub folder: String,
    /// The task its title reported, else the agent's label.
    pub task: String,
    /// When it was last seen, in seconds since the epoch.
    pub seen: i64,
}

impl Conversation {
    /// Whether every value is one `Recent::record` could have been given: a real id, a single
    /// line of command, an absolute folder, and words to show.
    fn is_sound(&self) -> bool {
        crate::terminal::resume::is_conversation_id(&self.id)
            && !self.agent.is_empty()
            && !self.task.is_empty()
            && !self.command.is_empty()
            && !self.command.chars().any(char::is_control)
            && Path::new(&self.folder).is_absolute()
    }
}

/// The kept conversations, most recent first, each id once.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recent {
    conversations: Vec<Conversation>,
}

impl Recent {
    /// The list `text` holds, or an empty one when it is not a list written here.
    pub fn parse(text: &str) -> Self {
        let Ok(read) = serde_json::from_str::<Self>(text) else {
            return Self::default();
        };
        let sound = read.conversations.len() <= KEPT
            && read.conversations.iter().all(Conversation::is_sound)
            && read.conversations.iter().enumerate().all(|(at, each)| {
                read.conversations[..at]
                    .iter()
                    .all(|earlier| earlier.id != each.id)
            });
        if sound {
            read
        } else {
            Self::default()
        }
    }

    /// The list as its file holds it.
    pub fn written(&self) -> String {
        serde_json::to_string_pretty(self).expect("a list of texts and numbers is JSON")
    }

    /// Puts `conversation` first, in place of what was kept under its id, and drops the oldest
    /// beyond `KEPT`.
    pub fn record(&mut self, conversation: Conversation) {
        self.conversations.retain(|kept| kept.id != conversation.id);
        self.conversations.insert(0, conversation);
        self.conversations.truncate(KEPT);
    }

    /// Drops the conversation `id`. Returns whether it was kept.
    pub fn forget(&mut self, id: &str) -> bool {
        let before = self.conversations.len();
        self.conversations.retain(|kept| kept.id != id);
        self.conversations.len() != before
    }

    /// The conversation kept under `id`.
    pub fn find(&self, id: &str) -> Option<&Conversation> {
        self.conversations.iter().find(|kept| kept.id == id)
    }

    /// The conversations the menu lists: the first `SHOWN` that are not among `running`, the ids
    /// of those open in a terminal now.
    pub fn shown(&self, running: &[String]) -> Vec<&Conversation> {
        self.conversations
            .iter()
            .filter(|kept| !running.contains(&kept.id))
            .take(SHOWN)
            .collect()
    }
}

/// Where the list is kept in the drive's app data `metadata`.
fn file(metadata: &Path) -> PathBuf {
    metadata.join(FILE)
}

/// The list kept in `metadata`; empty when none was kept or it does not read.
pub fn read(metadata: &Path) -> Recent {
    fs::read_to_string(file(metadata))
        .map_or_else(|_| Recent::default(), |text| Recent::parse(&text))
}

/// Keeps `recent` in `metadata`, replacing the file whole.
pub fn keep(metadata: &Path, recent: &Recent) -> std::io::Result<()> {
    fs::create_dir_all(metadata)?;
    let path = file(metadata);
    let staged = path.with_extension("writing");
    fs::write(&staged, recent.written())?;
    fs::rename(staged, path)
}

/// One row of the menu's "Recent" group.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// The conversation's id.
    pub id: String,
    /// The row's title: the conversation's task.
    pub task: String,
    /// The muted second line (`detail`).
    pub detail: String,
}

/// A row's second line: the folder as the sessions list writes it, then when the conversation
/// was last seen. `when` is empty when the day could not be told, and is then left out.
pub fn detail(folder: &str, when: &str) -> String {
    if when.is_empty() {
        folder.to_owned()
    } else {
        format!("{folder} · {when}")
    }
}

const MINUTE: i64 = 60;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// How long ago something `ago` seconds old was seen, as few words as say it: "now" under a
/// minute, then minutes ("5m"), hours ("2h"), the weekday within a week ("Tue"), then the day
/// and month ("3 Nov"). `seen` is the local day it was seen on, read only past a day ago; `None`
/// when the computer could not tell it, which leaves an older moment unsaid.
pub(crate) fn when_words(ago: i64, seen: Option<&Local>, language: &str) -> String {
    let counted = |words: strings::Key, count: i64| {
        strings::format(language, words, &[("count", &count.to_string())])
    };
    if ago < MINUTE {
        return strings::text(language, key::SESSIONS_RECENT_NOW).to_owned();
    }
    if ago < HOUR {
        return counted(key::SESSIONS_RECENT_MINUTES, ago / MINUTE);
    }
    if ago < DAY {
        return counted(key::SESSIONS_RECENT_HOURS, ago / HOUR);
    }
    let Some(seen) = seen else {
        return String::new();
    };
    if ago < 7 * DAY {
        crate::list::weekday_of(seen.day_number).to_owned()
    } else {
        format!("{} {}", seen.day, crate::list::month_of(seen.month))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str, seen: i64) -> Conversation {
        Conversation {
            id: id.to_owned(),
            agent: "Claude Code".to_owned(),
            command: format!("cd '/drive/Sales' && claude --resume {id}"),
            folder: "/drive/Sales".to_owned(),
            task: "Draft the Q4 outreach".to_owned(),
            seen,
        }
    }

    fn ids(listed: &[&Conversation]) -> Vec<String> {
        listed.iter().map(|each| each.id.clone()).collect()
    }

    #[test]
    fn a_recorded_conversation_comes_first_and_replaces_its_earlier_self() {
        let mut recent = Recent::default();
        recent.record(conversation("a", 10));
        recent.record(conversation("b", 20));
        let later = Conversation {
            task: "Send the outreach".to_owned(),
            ..conversation("a", 30)
        };
        recent.record(later.clone());
        assert_eq!(ids(&recent.shown(&[])), ["a", "b"]);
        assert_eq!(recent.find("a"), Some(&later));
        assert_eq!(recent.find("c"), None);
    }

    #[test]
    fn only_the_latest_are_kept_and_the_first_few_not_running_are_shown() {
        let mut recent = Recent::default();
        for number in 0..KEPT + 3 {
            recent.record(conversation(&format!("c{number}"), number as i64));
        }
        assert_eq!(recent.conversations.len(), KEPT);
        assert_eq!(recent.find("c2"), None, "the oldest are dropped");
        assert_eq!(ids(&recent.shown(&[])), ["c22", "c21", "c20", "c19", "c18"]);
        // A conversation open in a terminal now is left out, and the next one takes its place.
        let running = ["c22".to_owned(), "c20".to_owned()];
        assert_eq!(
            ids(&recent.shown(&running)),
            ["c21", "c19", "c18", "c17", "c16"]
        );
    }

    #[test]
    fn a_forgotten_conversation_is_gone() {
        let mut recent = Recent::default();
        recent.record(conversation("a", 10));
        recent.record(conversation("b", 20));
        assert!(recent.forget("a"));
        assert!(!recent.forget("a"));
        assert_eq!(ids(&recent.shown(&[])), ["b"]);
    }

    #[test]
    fn the_list_is_kept_in_the_drive_s_app_data_and_read_back() {
        let metadata = tempfile::tempdir().expect("metadata");
        let kept_in = metadata.path().join("not-made-yet");
        assert_eq!(read(&kept_in), Recent::default());
        let mut recent = Recent::default();
        recent.record(conversation("a", 10));
        recent.record(conversation("b-2_c", 20));
        keep(&kept_in, &recent).expect("kept");
        assert_eq!(read(&kept_in), recent);
        keep(&kept_in, &Recent::default()).expect("kept");
        assert_eq!(read(&kept_in), Recent::default());
    }

    #[test]
    fn a_file_that_was_not_written_here_counts_as_empty() {
        let mut recent = Recent::default();
        recent.record(conversation("a", 10));
        assert_eq!(Recent::parse(&recent.written()), recent);
        let with = |change: fn(&mut Conversation)| {
            let mut changed = conversation("a", 10);
            change(&mut changed);
            Recent {
                conversations: vec![changed],
            }
            .written()
        };
        let empty = Recent::default();
        assert_eq!(Recent::parse(""), empty);
        assert_eq!(Recent::parse("[]"), empty);
        assert_eq!(Recent::parse(r#"{"conversations": [{"id": "a"}]}"#), empty);
        assert_eq!(
            Recent::parse(r#"{"conversations": [], "version": 2}"#),
            empty
        );
        // An id that is not one, a command of two lines, a folder that is not a whole path.
        assert_eq!(Recent::parse(&with(|c| c.id = "a; rm".to_owned())), empty);
        assert_eq!(
            Recent::parse(&with(|c| c.command = "claude\nrm".to_owned())),
            empty
        );
        assert_eq!(
            Recent::parse(&with(|c| c.folder = "Sales".to_owned())),
            empty
        );
        assert_eq!(Recent::parse(&with(|c| c.task = String::new())), empty);
        // The same id twice, or more than are ever kept.
        let twice = Recent {
            conversations: vec![conversation("a", 10), conversation("a", 20)],
        };
        assert_eq!(Recent::parse(&twice.written()), empty);
        let many = Recent {
            conversations: (0..=KEPT)
                .map(|number| conversation(&format!("c{number}"), 0))
                .collect(),
        };
        assert_eq!(Recent::parse(&many.written()), empty);
    }

    #[test]
    fn when_is_said_in_minutes_then_hours_then_the_weekday_then_the_date() {
        // 3 November 2026 is a Tuesday.
        let seen = Local {
            year: 2026,
            month: 10,
            day: 3,
            hour: 9,
            minute: 0,
            second: 0,
            day_number: crate::calendar::days_from_civil(2026, 11, 3),
        };
        let said = |ago: i64| when_words(ago, Some(&seen), "en");
        assert_eq!(said(0), "now");
        assert_eq!(said(-30), "now", "a clock set back is still now");
        assert_eq!(said(59), "now");
        assert_eq!(said(60), "1m");
        assert_eq!(said(5 * 60 + 59), "5m");
        assert_eq!(said(HOUR), "1h");
        assert_eq!(said(2 * HOUR + 59 * 60), "2h");
        assert_eq!(said(DAY), "Tue");
        assert_eq!(said(7 * DAY - 1), "Tue");
        assert_eq!(said(7 * DAY), "3 Nov");
        assert_eq!(when_words(HOUR, None, "en"), "1h");
        assert_eq!(when_words(DAY, None, "en"), "");
    }

    #[test]
    fn a_row_s_second_line_is_its_folder_then_when() {
        assert_eq!(detail("Sales / Q4 leads", "5m"), "Sales / Q4 leads · 5m");
        assert_eq!(detail("Sales", ""), "Sales");
    }
}
