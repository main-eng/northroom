//! The messages a phone and this computer exchange, each one JSON object with a `type`. They are
//! an outside contract (`docs/phone-link.md`): other people's devices speak them too, so a
//! change an older device can't read takes the link's next protocol number
//! (`northroom_link::PROTOCOL`).

use crate::remote::screen::Screen;
use crate::sessions::Running;
use crate::terminal::activity::Activity;
use crate::terminal::input::Key;
use serde::{Deserialize, Serialize};

/// What a device asks.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Ask {
    /// Pair this device, proving it read the code this computer showed. `name` is what the
    /// person will see the device listed as.
    Pair { code: String, name: String },
    /// A paired device has connected and wants the sessions.
    Hello,
    /// Show `session`'s terminal, fitted to a grid of this size, until another `watch` or a
    /// `leave`.
    Watch {
        session: String,
        columns: usize,
        lines: usize,
    },
    /// Stop watching.
    Leave,
    /// Type `text` into `session`, as a paste.
    Type { session: String, text: String },
    /// Press one key in `session`: `enter`, `escape`, `tab`, `backspace`, `up`, `down`, `left`,
    /// `right` or `interrupt` (Control-C).
    Press { session: String, key: String },
}

/// Why a device was turned away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    /// The device is not paired and did not ask to pair.
    NotPaired,
    /// The pairing code is wrong, used or older than it may be.
    WrongCode,
    /// The message could not be read.
    NotUnderstood,
}

/// One running session, as `sessions::Running` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub doing: String,
    pub agent: String,
    /// `needs-you`, `finished`, `working` or `idle`.
    pub activity: &'static str,
}

impl Session {
    pub fn of(running: &Running) -> Self {
        Self {
            id: running.id.clone(),
            name: running.name.clone(),
            folder: running.folder.clone(),
            doing: running.doing.clone(),
            agent: running.agent.clone(),
            activity: activity_word(running.activity),
        }
    }
}

/// The word a device reads for `activity`. A starting session reads as idle, as the sessions
/// list groups it, and an ended one is not listed.
fn activity_word(activity: Activity) -> &'static str {
    match activity {
        Activity::NeedsYou => "needs-you",
        Activity::Finished => "finished",
        Activity::Working => "working",
        Activity::Starting | Activity::Idle | Activity::Ended => "idle",
    }
}

/// What this computer tells a device.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Tell {
    /// The device may stay. `host` is this computer's name.
    Welcome { host: String },
    /// The device may not; the link closes after this.
    Refused { why: Refusal },
    /// Every running session, in the order the sessions list shows them. Sent after `hello` and
    /// again whenever it changes.
    Sessions { sessions: Vec<Session> },
    /// The watched session's screen. Sent after `watch` and again whenever it changes.
    Screen { session: String, screen: Screen },
    /// The watched session has ended or its terminal is gone.
    Gone { session: String },
}

impl Tell {
    pub fn encoded(&self) -> Vec<u8> {
        // Every field is a string, a number, a flag or a list of them, which always encode.
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// Reads a device's message. `None` for anything that is not one.
pub fn parse_ask(bytes: &[u8]) -> Option<Ask> {
    serde_json::from_slice(bytes).ok()
}

/// What pressing the key named `name` sends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    Key(Key),
    /// Control-C.
    Interrupt,
}

/// Reads a `press` message's key name.
pub fn parse_key(name: &str) -> Option<Pressed> {
    Some(Pressed::Key(match name {
        "enter" => Key::Return,
        "escape" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "interrupt" => return Some(Pressed::Interrupt),
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_s_messages_are_read_by_their_type() {
        assert_eq!(parse_ask(br#"{"type":"hello"}"#), Some(Ask::Hello));
        assert_eq!(
            parse_ask(br#"{"type":"pair","code":"abc","name":"My phone"}"#),
            Some(Ask::Pair {
                code: "abc".to_owned(),
                name: "My phone".to_owned()
            })
        );
        assert_eq!(
            parse_ask(br#"{"type":"watch","session":"s1","columns":48,"lines":30}"#),
            Some(Ask::Watch {
                session: "s1".to_owned(),
                columns: 48,
                lines: 30
            })
        );
        assert_eq!(parse_ask(br#"{"type":"leave"}"#), Some(Ask::Leave));
    }

    #[test]
    fn anything_else_is_not_a_message() {
        assert_eq!(parse_ask(b"hello"), None);
        assert_eq!(parse_ask(br#"{"type":"format-disk"}"#), None);
        assert_eq!(parse_ask(br#"{"type":"watch","session":"s1"}"#), None);
    }

    #[test]
    fn what_a_device_is_told_names_its_type_in_words() {
        let told = Tell::Refused {
            why: Refusal::WrongCode,
        };
        assert_eq!(
            String::from_utf8(told.encoded()).unwrap(),
            r#"{"type":"refused","why":"wrong-code"}"#
        );
    }

    #[test]
    fn a_session_reads_as_the_sessions_list_groups_it() {
        let running = Running {
            id: "s1".to_owned(),
            name: "Build".to_owned(),
            folder: "Work".to_owned(),
            doing: "Fixing the tests".to_owned(),
            agent: "Claude Code".to_owned(),
            activity: Activity::Starting,
        };
        assert_eq!(Session::of(&running).activity, "idle");
        assert_eq!(activity_word(Activity::NeedsYou), "needs-you");
    }

    #[test]
    fn only_the_listed_keys_can_be_pressed() {
        assert_eq!(parse_key("enter"), Some(Pressed::Key(Key::Return)));
        assert_eq!(parse_key("interrupt"), Some(Pressed::Interrupt));
        assert_eq!(parse_key("f13"), None);
    }
}
