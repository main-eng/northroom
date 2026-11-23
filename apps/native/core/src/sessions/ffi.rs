//! Running sessions (`sessions`) for the shell: where each sits in the drive, for the title
//! bar's sessions menu, and what the sessions button shows.

use crate::ffi::with_columns;
use crate::sessions::{self, Running, Signal};
use crate::terminal::activity::{look, Activity, Sign};
use crate::theme::Token;
use crate::workspace::Workspace;
use crate::Relock;

/// What every listed session is doing (`sessions::listed_activity`), the title its program set
/// and the agent running in it, by its terminal file's id. A terminal with nothing running in it
/// is left out.
fn session_states() -> Vec<(String, Activity, String, &'static str)> {
    crate::app()
        .sessions
        .relock()
        .iter()
        .filter_map(|(id, session)| {
            let listed = sessions::listed_activity(
                session.activity(),
                session.agent().is_some(),
                session.busy(),
            )?;
            let agent = session.agent().map_or("", |agent| agent.label());
            Some((id.clone(), listed, session.title(), agent))
        })
        .collect()
}

/// A file's folder path as the sessions list writes it, joined with " / ", or the drive's name
/// for a file at the top of the drive.
fn folder_words(folders: &[crate::workspace::Crumb]) -> String {
    let below: Vec<&str> = folders
        .iter()
        .skip(1)
        .map(|crumb| crumb.title.as_str())
        .collect();
    match (below.is_empty(), folders.first()) {
        (true, Some(drive)) => drive.title.clone(),
        _ => below.join(" / "),
    }
}

/// The session of terminal file `id` as the list shows it. `None` when the file is not in the
/// drive, for example after it was trashed while its session ran.
fn placed_session(
    workspace: &mut Workspace,
    id: String,
    activity: Activity,
    title: &str,
    agent: &str,
) -> Option<Running> {
    let placed = workspace.place_of(&id).ok()?;
    Some(Running {
        id,
        name: placed.title,
        folder: folder_words(&placed.folders),
        doing: sessions::doing_of(title),
        agent: agent.to_owned(),
        activity,
    })
}

/// Every session running now. Empty before a drive is open.
pub fn running_now() -> Vec<Running> {
    // Read and release the sessions before asking the drive where their files are.
    let states = session_states();
    with_columns(|columns| {
        let workspace = columns.workspace();
        states
            .into_iter()
            .filter_map(|(id, activity, title, agent)| {
                placed_session(workspace, id, activity, &title, agent)
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The longest task a tab shows before it is cut with "…".
const TAB_TASK_CHARACTERS: usize = 32;

/// The task the session of terminal file `id` reports (`sessions::task_of`), shortened for a tab,
/// or `None` when it reports none or nothing runs there. `name` is the terminal's own name.
pub fn tab_task(id: &str, name: &str) -> Option<String> {
    let (title, agent) = {
        let running = crate::app().sessions.relock();
        let session = running.get(id)?;
        (
            session.title(),
            session.agent().map_or("", |agent| agent.label()),
        )
    };
    let session = Running {
        id: id.to_owned(),
        name: name.to_owned(),
        folder: String::new(),
        doing: sessions::doing_of(&title),
        agent: agent.to_owned(),
        activity: Activity::Working,
    };
    sessions::task_of(&session).map(|task| sessions::shortened(task, TAB_TASK_CHARACTERS))
}

/// The title bar's sessions button as it stands (`sessions::Signal`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionsButton {
    /// The button's text, such as "2 need you", drawn as a pill in the colour of a session that
    /// needs the person. Empty when none does, leaving only the glyph.
    pub words: String,
    /// The dot at the glyph's corner, as on a card (`terminal::activity::Look`): a session finished
    /// unseen, or one is working. `None` for no dot.
    pub dot: Option<Token>,
    /// The shape the dot takes.
    pub sign: Sign,
    /// No session is running, so the glyph is drawn muted.
    pub quiet: bool,
}

/// Every session, counted by activity only, so the drive is not asked where the files are.
fn counted_sessions() -> Vec<Running> {
    session_states()
        .into_iter()
        .map(|(id, activity, _, _)| Running {
            id,
            name: String::new(),
            folder: String::new(),
            doing: String::new(),
            agent: String::new(),
            activity,
        })
        .collect()
}

/// The button for what `signal` says, in `language`.
fn button_of(signal: Signal, language: &str) -> SessionsButton {
    let dotted = |activity: Activity| {
        let shown = look(Some(activity));
        (String::new(), shown.dot, shown.sign)
    };
    let (words, dot, sign) = match signal {
        Signal::NeedsYou(sessions) => {
            (sessions::needing_words(sessions, language), None, Sign::Dot)
        }
        Signal::Finished => dotted(Activity::Finished),
        Signal::Working => dotted(Activity::Working),
        Signal::Idle | Signal::Nothing => (String::new(), None, Sign::Dot),
    };
    SessionsButton {
        words,
        dot,
        sign,
        quiet: signal == Signal::Nothing,
    }
}

/// What the title bar's sessions button says now.
pub fn sessions_button() -> SessionsButton {
    button_of(
        sessions::signal(&counted_sessions()),
        crate::ffi::interface_language(),
    )
}

/// What the sessions button says when the pointer rests on it (`sessions::summary`).
pub fn sessions_summary() -> String {
    sessions::summary(&counted_sessions(), crate::ffi::interface_language())
}

/// How many sessions wait for the person (`sessions::waiting`), for the Dock's icon.
pub fn sessions_waiting() -> usize {
    session_states()
        .iter()
        .filter(|(_, activity, _, _)| matches!(activity, Activity::NeedsYou | Activity::Finished))
        .count()
}

/// A notice for the system's notification centre: a session came to wait for the person
/// while they were not looking at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionNotice {
    /// The sessions menu's entry for it (`menus::session_entry`): what choosing the notice
    /// runs, and what a later notice of the same session replaces.
    pub entry: String,
    /// The session's task, or the terminal's name when it reports none (`sessions::heading_of`).
    pub title: String,
    /// What it asks, and the folder it is in.
    pub body: String,
}

/// Sessions that started needing the person, or finished, since the last call
/// (`sessions::Watch`). `at_window` says whether the person is at the app's window, in which
/// case the session on screen is not news.
pub fn session_news(at_window: bool) -> Vec<SessionNotice> {
    let app = crate::app();
    let states: Vec<(String, Activity)> = session_states()
        .into_iter()
        .map(|(id, activity, _, _)| (id, activity))
        .collect();
    let on_screen = app
        .shown_terminals
        .relock()
        .front_file()
        .map(str::to_owned)
        .filter(|_| at_window);
    let news = app
        .session_watch
        .relock()
        .take_news(&states, on_screen.as_deref());
    if news.is_empty() {
        return Vec::new();
    }
    let language = crate::ffi::interface_language();
    running_now()
        .into_iter()
        .filter(|session| news.contains(&session.id))
        .map(|session| SessionNotice {
            entry: crate::menus::session_entry_of(&session.id),
            body: format!("{} · {}", session.activity.label(language), session.folder),
            title: sessions::heading_of(&session),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Crumb, Location};

    #[test]
    fn the_button_is_a_pill_of_words_only_while_a_session_needs_the_person() {
        let needing = button_of(Signal::NeedsYou(2), "en");
        assert_eq!((needing.words.as_str(), needing.dot), ("2 need you", None));
        assert_eq!(
            button_of(Signal::Finished, "en"),
            SessionsButton {
                words: String::new(),
                dot: Some(Token::StatusSuccess),
                sign: Sign::Check,
                quiet: false,
            }
        );
        let working = button_of(Signal::Working, "en");
        assert_eq!(
            (working.dot, working.sign),
            (Some(Token::StatusInfo), Sign::Comet)
        );
        let idle = button_of(Signal::Idle, "en");
        assert_eq!((idle.dot, idle.quiet), (None, false));
        assert!(button_of(Signal::Nothing, "en").quiet);
    }

    #[test]
    fn a_sessions_folder_is_written_from_under_the_drive_down() {
        let crumb = |title: &str| Crumb {
            location: Location::ROOT,
            title: title.to_owned(),
        };
        assert_eq!(folder_words(&[crumb("My Drive")]), "My Drive");
        assert_eq!(
            folder_words(&[crumb("My Drive"), crumb("Sales"), crumb("Q4 leads")]),
            "Sales / Q4 leads"
        );
    }
}
