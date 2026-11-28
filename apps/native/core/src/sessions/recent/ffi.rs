//! Recent conversations (`sessions::recent`) for the app: recording the ones its terminals
//! hold into the open drive's list, and reading that list for the sessions menu.

use crate::ffi::with_columns;
use crate::sessions::recent::{self, Conversation, Recent, Row};
use crate::terminal::activity::Resumable;
use crate::terminal::Terminal;
use crate::workspace::Workspace;
use crate::Relock;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The list of the drive whose app data is at the path beside it, read from its file once and
/// written back on every change.
static KEPT: Mutex<Option<(PathBuf, Recent)>> = Mutex::new(None);

/// Runs `action` on the list kept in `metadata`.
fn with_list<T>(metadata: &Path, action: impl FnOnce(&mut Recent) -> T) -> T {
    let mut kept = KEPT.relock();
    if kept.as_ref().is_some_and(|(of, _)| of != metadata) {
        *kept = None;
    }
    let (_, list) = kept.get_or_insert_with(|| (metadata.to_path_buf(), recent::read(metadata)));
    action(list)
}

/// Changes the list kept in `metadata` with `action` and writes it to its file; a failure to
/// write is only reported, and the list stays as changed until the app quits.
fn change_list<T>(metadata: &Path, action: impl FnOnce(&mut Recent) -> T) -> T {
    with_list(metadata, |list| {
        let done = action(list);
        if let Err(error) = recent::keep(metadata, list) {
            eprintln!("the recent conversations were not kept: {error}");
        }
        done
    })
}

/// The open drive's app data folder. `None` before a drive is open.
fn metadata() -> Option<PathBuf> {
    with_columns(|columns| columns.workspace().metadata_directory().to_path_buf())
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// A conversation a terminal holds, to record.
struct Held {
    /// The terminal file's id.
    file: String,
    conversation: Resumable,
    /// The conversation is the one open in the terminal now, not one that left it.
    open: bool,
}

/// What `terminals`, by their files' ids, hold: the conversations that left each, then the one
/// open in it. Those that left are taken, so each is held once.
fn held_by<'a>(terminals: impl IntoIterator<Item = (&'a str, &'a Terminal)>) -> Vec<Held> {
    terminals
        .into_iter()
        .flat_map(|(file, terminal)| {
            let held = |open: bool| {
                move |conversation: Resumable| Held {
                    file: file.to_owned(),
                    conversation,
                    open,
                }
            };
            let left = terminal
                .take_left_conversations()
                .into_iter()
                .map(held(false));
            left.chain(terminal.resumable().map(held(true)))
        })
        .collect()
}

/// `held` as the list keeps it, seen at `seen`, for a terminal called `name`: its task is what
/// its title reported (`sessions::task_said`), else its agent's label.
fn conversation_of(held: Resumable, name: &str, seen: i64) -> Conversation {
    let agent = held.agent.label();
    let doing = crate::sessions::doing_of(&held.title);
    let task = crate::sessions::task_said(&doing, name, agent).unwrap_or(agent);
    Conversation {
        task: task.to_owned(),
        id: held.id,
        agent: agent.to_owned(),
        command: held.command,
        folder: held.folder,
        seen,
    }
}

/// Records `held` in the open drive's list: every conversation that left its terminal, and each
/// open one when `ending` (its terminal is closing) or when the list does not hold it yet.
fn record(held: Vec<Held>, ending: bool) {
    if held.is_empty() {
        return;
    }
    let Some(metadata) = metadata() else {
        return;
    };
    let wanted: Vec<Held> = with_list(&metadata, |list| {
        held.into_iter()
            .filter(|held| !held.open || ending || list.find(&held.conversation.id).is_none())
            .collect()
    });
    if wanted.is_empty() {
        return;
    }
    // A terminal already in the Trash has no name to tell from its title.
    let names: Vec<String> = with_columns(|columns| {
        let workspace = columns.workspace();
        wanted
            .iter()
            .map(|held| {
                workspace
                    .place_of(&held.file)
                    .map_or_else(|_| String::new(), |placed| placed.title)
            })
            .collect()
    })
    .unwrap_or_default();
    let seen = now_seconds();
    change_list(&metadata, |list| {
        for (held, name) in wanted.into_iter().zip(names) {
            list.record(conversation_of(held.conversation, &name, seen));
        }
    });
}

/// Records the conversations that left a terminal since the last call, and those open now that
/// the list does not hold yet. Called whenever a session changes (`session_news`).
pub(crate) fn keep_changed() {
    let held = {
        let sessions = crate::app().sessions.relock();
        held_by(
            sessions
                .iter()
                .map(|(id, terminal)| (id.as_str(), terminal)),
        )
    };
    record(held, false);
}

/// Records every conversation `terminals` hold, by their files' ids, as last seen now. Called
/// as their sessions end, while their files are still in the drive.
pub(crate) fn keep_ending<'a>(terminals: impl IntoIterator<Item = (&'a str, &'a Terminal)>) {
    record(held_by(terminals), true);
}

/// The ids of the conversations open in a terminal now, known from each agent's hook.
fn running() -> Vec<String> {
    let sessions = crate::app().sessions.relock();
    sessions
        .values()
        .filter_map(Terminal::conversation)
        .collect()
}

/// `folder`, an absolute path, as the sessions list writes folders
/// (`sessions::ffi::folder_words`). One outside the drive, or gone, is its last name.
fn folder_words(workspace: &mut Workspace, folder: &str) -> String {
    let path = Path::new(folder);
    let inside = workspace
        .location_at(path)
        .and_then(|location| workspace.path(&location));
    match inside {
        Ok(folders) => crate::sessions::ffi::folder_words(&folders),
        Err(_) => path.file_name().map_or_else(
            || folder.to_owned(),
            |name| name.to_string_lossy().into_owned(),
        ),
    }
}

/// The rows of the sessions menu's "Recent" group (`Recent::shown`). Empty before a drive is
/// open.
pub fn recent_rows() -> Vec<Row> {
    let Some(metadata) = metadata() else {
        return Vec::new();
    };
    let running = running();
    let shown: Vec<Conversation> = with_list(&metadata, |list| {
        list.shown(&running).into_iter().cloned().collect()
    });
    if shown.is_empty() {
        return Vec::new();
    }
    let now = now_seconds();
    let language = crate::ffi::interface_language();
    with_columns(|columns| {
        let workspace = columns.workspace();
        shown
            .into_iter()
            .map(|conversation| {
                let when = recent::when_words(
                    now - conversation.seen,
                    crate::list::local(conversation.seen).as_ref(),
                    language,
                );
                Row {
                    detail: recent::detail(&folder_words(workspace, &conversation.folder), &when),
                    id: conversation.id,
                    task: conversation.task,
                }
            })
            .collect()
    })
    .unwrap_or_default()
}

/// The conversation the open drive's list keeps under `id`.
pub(crate) fn recent_conversation(id: &str) -> Option<Conversation> {
    with_list(&metadata()?, |list| list.find(id).cloned())
}

/// Drops conversation `id` from the open drive's list. Returns whether it was kept.
pub(crate) fn forget_recent(id: &str) -> bool {
    metadata().is_some_and(|metadata| change_list(&metadata, |list| list.forget(id)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::agent::Agent;

    fn held(id: &str, title: &str) -> Resumable {
        Resumable {
            id: id.to_owned(),
            agent: Agent::Claude,
            command: format!("cd '/drive/Sales' && claude --resume {id}"),
            folder: "/drive/Sales".to_owned(),
            title: title.to_owned(),
        }
    }

    #[test]
    fn a_conversation_is_kept_under_its_task_or_else_its_agent() {
        let kept = conversation_of(held("a", "✳ Draft the outreach"), "Terminal", 40);
        assert_eq!(
            (kept.task.as_str(), kept.agent.as_str(), kept.seen),
            ("Draft the outreach", "Claude Code", 40)
        );
        assert_eq!(kept.command, "cd '/drive/Sales' && claude --resume a");
        // A title that only names the program, or says nothing, is no task.
        assert_eq!(
            conversation_of(held("a", "Claude Code"), "", 0).task,
            "Claude Code"
        );
        assert_eq!(
            conversation_of(held("a", ""), "Terminal", 0).task,
            "Claude Code"
        );
        // What is kept reads back from its file.
        let mut list = Recent::default();
        list.record(kept);
        assert_eq!(Recent::parse(&list.written()), list);
    }

    #[test]
    fn a_drive_s_list_is_read_once_and_written_on_every_change() {
        let metadata = tempfile::tempdir().expect("metadata");
        change_list(metadata.path(), |list| {
            list.record(conversation_of(held("a", ""), "", 10));
            list.record(conversation_of(held("b", ""), "", 20));
        });
        assert_eq!(recent::read(metadata.path()).shown(&[]).len(), 2);
        assert!(change_list(metadata.path(), |list| list.forget("a")));
        let on_disk = recent::read(metadata.path());
        assert_eq!(on_disk.find("a"), None);
        assert!(on_disk.find("b").is_some());
        assert!(with_list(metadata.path(), |list| *list == on_disk));
    }

    #[test]
    fn a_folder_is_written_as_the_sessions_list_writes_it_or_by_its_last_name() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        let inside = root.path().join("Sales").join("Q4 leads");
        std::fs::create_dir_all(&inside).expect("folders");
        let mut workspace = Workspace::open(root.path(), metadata.path()).expect("open");
        assert_eq!(
            folder_words(&mut workspace, &inside.to_string_lossy()),
            "Sales / Q4 leads"
        );
        let drive = workspace
            .path(&crate::workspace::Location::ROOT)
            .expect("the drive")[0]
            .title
            .clone();
        assert_eq!(
            folder_words(&mut workspace, &root.path().to_string_lossy()),
            drive
        );
        // Gone, or outside the drive.
        assert_eq!(
            folder_words(
                &mut workspace,
                &root.path().join("Old work").to_string_lossy()
            ),
            "Old work"
        );
        assert_eq!(
            folder_words(&mut workspace, &metadata.path().to_string_lossy()),
            metadata
                .path()
                .file_name()
                .expect("a name")
                .to_string_lossy()
        );
    }
}
