//! The Changes tab's model: the files of a root in one flat list, each with its diff.
//!
//! It lists every file the terminals changed since the person last kept it, newest first.
//! Keep takes a file's change, or one hunk of it, into what the file is diffed from (as Zed's
//! review keeps an agent's edits): a whole file leaves the list and lists again only once a
//! terminal changes it after that; a kept hunk leaves the diff and the file's other hunks stay.
//! Changes made while no turn ran are credited to no terminal and left out. A file an earlier
//! run listed (`Root::carried`) lists on until it is kept or is again as it was; a change made
//! while the app was closed keeps the credit it had. The shell only draws what this lists.
//!
//! Every file starts expanded except noisy ones (`starts_folded`): large diffs, lock and
//! generated files, deleted files and files without a text diff. A file the person collapsed
//! or expanded stays so while the tab is open.

use crate::changes::actions::ActionError;
use crate::changes::diff::{self, Deadline, FileDiff};
use crate::changes::record::{Listed, Record};
use crate::changes::scan::Stamp;
use crate::changes::snapshot::{Content, Entry, Manifest};
use crate::changes::store::{Hash, Store};
use crate::changes::turns::{Engine, Root, Turn};
use crate::changes::Clock;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

/// How a root's Changes tab is set; kept while its tab is open, wherever the window goes.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    /// The path of the file in view: the one at the top of the diff, which the tools act on.
    pub selected: Option<String>,
    /// How far the diff is scrolled into that file's section, in points.
    pub within: f64,
    /// Each file the person collapsed (true) or expanded (false), by path; wins over how it
    /// starts (`folded`).
    pub folds: BTreeMap<String, bool>,
}

/// What happened to a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
}

/// One file's change between two snapshots; `None` where it did not exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    pub before: Option<Entry>,
    pub after: Option<Entry>,
}

impl FileChange {
    pub fn status(&self) -> Status {
        match (&self.before, &self.after) {
            (None, _) => Status::Added,
            (_, None) => Status::Deleted,
            (Some(_), Some(_)) => Status::Modified,
        }
    }

    /// Whether it has a line diff: both sides are text or absent.
    pub fn is_text(&self) -> bool {
        self.text_pair().is_some()
    }

    /// The text hashes on each side, when both sides are text or absent and one is present.
    fn text_pair(&self) -> Option<Pair> {
        let text = |entry: &Option<Entry>| match entry.as_ref().map(|entry| &entry.content) {
            None => Some(None),
            Some(Content::Text(hash)) => Some(Some(hash.clone())),
            Some(Content::Binary(_) | Content::TooLarge | Content::Unread) => None,
        };
        Some((text(&self.before)?, text(&self.after)?))
    }
}

/// Whether a file is different from `before` to `after`.
fn differs(before: Option<&Entry>, after: Option<&Entry>) -> bool {
    match (before, after) {
        (Some(before), Some(after)) => before.differs(after),
        (None, None) => false,
        _ => true,
    }
}

/// The files that differ from `base` to `end`, by path.
pub fn changes_between(base: &Manifest, end: &Manifest) -> Vec<FileChange> {
    let paths: BTreeSet<&String> = base.files.keys().chain(end.files.keys()).collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let (before, after) = (base.files.get(path), end.files.get(path));
            differs(before, after).then(|| FileChange {
                path: path.clone(),
                before: before.cloned(),
                after: after.cloned(),
            })
        })
        .collect()
}

/// A file's text on each side, by hash.
type Pair = (Option<Hash>, Option<Hash>);

/// Lines added and removed for each pair of texts, filled in off the UI thread.
#[derive(Default)]
pub struct Counts(HashMap<Pair, (usize, usize)>);

impl Counts {
    pub fn get(&self, change: &FileChange) -> Option<(usize, usize)> {
        self.0.get(&change.text_pair()?).copied()
    }

    pub fn insert(&mut self, pair: Pair, counts: (usize, usize)) {
        self.0.insert(pair, counts);
    }

    fn lacks(&self, change: &FileChange) -> Option<Pair> {
        change.text_pair().filter(|pair| !self.0.contains_key(pair))
    }
}

/// A row of the file list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    pub change: FileChange,
    pub status: Status,
    /// Lines added and removed, once counted; only for text.
    pub counts: Option<(usize, usize)>,
    /// Two terminals changed it at the same time.
    pub contested: bool,
    /// When the first turn that changed it since it was last kept started; orders the list.
    pub entered_ms: u64,
}

/// A file a terminal changed since it was last kept: the change from then to now, and when the
/// first turn that changed it since then started, which orders the list.
struct Unkept {
    change: FileChange,
    entered_ms: u64,
    contested: bool,
    /// The terminals credited, by `Turn::credit`.
    terminals: BTreeSet<String>,
    /// When a turn last changed it; `None` while a turn that changes it runs.
    changed_ms: Option<u64>,
}

/// Each turn with the paths it changed, a running turn's up to the latest snapshot.
fn turn_paths(root: &Root) -> Vec<(&Turn, BTreeSet<String>)> {
    root.turns
        .iter()
        .filter_map(|turn| {
            let end = turn.after.clone().or_else(|| root.latest.clone())?;
            let changed = changes_between(turn.before.as_ref()?, &end)
                .into_iter()
                .map(|change| change.path)
                .chain(turn.reported.iter().cloned())
                .collect();
            Some((turn, changed))
        })
        .collect()
}

/// Every turn whose snapshots differ at `path`, or that the hook told edited it, since
/// `since_ms`.
fn counted<'a>(
    changed: &[(&'a Turn, BTreeSet<String>)],
    path: &str,
    since_ms: u64,
) -> Vec<&'a Turn> {
    changed
        .iter()
        .filter(|(turn, paths)| {
            paths.contains(path) && turn.ended_ms.is_none_or(|ended| ended >= since_ms)
        })
        .map(|(turn, _)| *turn)
        .collect()
}

/// The turns of `counted` credited with a file: those the hook told edited it, else all.
fn credited<'a>(counted: &[&'a Turn], path: &str) -> Vec<&'a Turn> {
    let reported: Vec<&Turn> = counted
        .iter()
        .copied()
        .filter(|turn| turn.reported.contains(path))
        .collect();
    if reported.is_empty() {
        counted.to_vec()
    } else {
        reported
    }
}

/// Whether two of `turns`, of different terminals, ran at the same time.
fn contested(turns: &[&Turn]) -> bool {
    turns.iter().any(|one| {
        turns
            .iter()
            .any(|other| one.key.terminal != other.key.terminal && one.overlaps(other))
    })
}

/// `path`'s change since it was last kept, else since an earlier run's baseline (`carried`),
/// else since the session began, if it differs.
fn change_since_kept(
    root: &Root,
    base: &Manifest,
    latest: &Manifest,
    path: &str,
    carried: Option<&Listed>,
) -> Option<FileChange> {
    let before = match (root.kept.get(path), carried) {
        (Some(kept), _) => kept.entry.clone(),
        (None, Some(carried)) => carried.baseline.clone(),
        (None, None) => base.files.get(path).cloned(),
    };
    let after = latest.files.get(path).cloned();
    differs(before.as_ref(), after.as_ref()).then(|| FileChange {
        path: path.to_owned(),
        before,
        after,
    })
}

/// Every file of `root` a terminal changed since it was last kept, in this run or one an
/// earlier run listed, by path.
fn unkept(root: &Root) -> Vec<Unkept> {
    let (Some(base), Some(latest)) = (&root.session_base, &root.latest) else {
        return Vec::new();
    };
    let changed = turn_paths(root);
    let paths: BTreeSet<&String> = changed
        .iter()
        .flat_map(|(_, paths)| paths)
        .chain(root.carried.keys())
        .collect();
    paths
        .into_iter()
        .filter_map(|path| {
            let since_ms = root.kept.get(path).map_or(0, |kept| kept.since_ms);
            let counted = counted(&changed, path, since_ms);
            let carried = root.carried(path);
            // The first turn counted, reported or not, so a later report doesn't move it.
            let entered_ms = counted
                .iter()
                .map(|turn| turn.started_ms)
                .chain(carried.map(|carried| carried.entered_ms))
                .min()?;
            let change = change_since_kept(root, base, latest, path, carried)?;
            let credited = credited(&counted, path);
            let ended = counted.iter().map(|turn| turn.ended_ms);
            let carried_ms = carried.map(|carried| Some(carried.changed_ms));
            // A running turn (`None`) is the latest of all.
            let changed_ms = ended
                .chain(carried_ms)
                .collect::<Option<Vec<u64>>>()
                .map(|times| times.into_iter().max().unwrap_or(entered_ms));
            Some(Unkept {
                change,
                entered_ms,
                contested: contested(&credited) || carried.is_some_and(|carried| carried.contested),
                terminals: credited
                    .iter()
                    .map(|turn| turn.credit.clone())
                    .chain(
                        carried
                            .into_iter()
                            .flat_map(|carried| carried.terminals.clone()),
                    )
                    .collect(),
                changed_ms,
            })
        })
        .collect()
}

/// What `root`'s list needs across restarts as of `now_ms` (`record`): every file listed,
/// with its baseline and credit, and every kept mark. Before this run's first snapshot it is
/// what an earlier run left.
pub fn record(root: &Root, now_ms: u64) -> Record {
    let listed = if root.session_base.is_some() && root.latest.is_some() {
        unkept(root)
            .into_iter()
            .map(|file| {
                let listed = Listed {
                    baseline: file.change.before,
                    terminals: file.terminals,
                    entered_ms: file.entered_ms,
                    changed_ms: file.changed_ms.unwrap_or(now_ms),
                    contested: file.contested,
                };
                (file.change.path, listed)
            })
            .collect()
    } else {
        root.carried.clone()
    };
    let mut record = Record::new(listed, root.kept.clone());
    record.expire(now_ms);
    record
}

/// How many files `root`'s list holds, which the title bar's review button says.
/// Before this run's first snapshot of it, those an earlier run left.
pub fn waiting(engine: &Engine, root: &Path) -> usize {
    let Some(watched) = engine.roots.get(root) else {
        return 0;
    };
    if watched.session_base.is_none() || watched.latest.is_none() {
        return watched
            .carried
            .keys()
            .filter(|path| watched.carried(path).is_some())
            .count();
    }
    unkept(watched).len()
}

/// The kept objects every root's list diffs now, for pruning to spare.
pub fn referenced(engine: &Engine) -> BTreeSet<Hash> {
    let mut hashes = BTreeSet::new();
    for root in engine.roots.values() {
        let sides = unkept(root)
            .into_iter()
            .flat_map(|file| [file.change.before, file.change.after])
            .flatten();
        let kept = root.kept.values().filter_map(|mark| mark.entry.clone());
        let carried = root
            .carried
            .values()
            .filter_map(|listed| listed.baseline.clone());
        let entries = sides.chain(kept).chain(carried);
        hashes.extend(entries.filter_map(|entry| entry.content.hash().cloned()));
    }
    hashes
}

/// The file list of `root`: every file changed since it was last kept, newest first by when a
/// turn first changed it since then, then by path. A file changed again keeps its place, so the
/// list doesn't reshuffle while it is looked at, and a newly changed file comes in at the top.
pub fn list(engine: &Engine, root: &Path) -> Vec<FileRow> {
    let Some(watched) = engine.roots.get(root) else {
        return Vec::new();
    };
    let mut rows: Vec<FileRow> = unkept(watched)
        .into_iter()
        .map(|file| FileRow {
            status: file.change.status(),
            counts: engine.counts.get(&file.change),
            contested: file.contested,
            entered_ms: file.entered_ms,
            change: file.change,
        })
        .collect();
    rows.sort_by(|a, b| {
        (b.entered_ms.cmp(&a.entered_ms)).then_with(|| a.change.path.cmp(&b.change.path))
    });
    rows
}

/// Keeps the file at `path` of `root`'s list whole at `now_ms`: it leaves the list until a
/// terminal changes it again. Returns whether it was listed.
pub fn keep_file(engine: &mut Engine, root: &Path, path: &str, now_ms: u64) -> bool {
    let Some(row) = list(engine, root)
        .into_iter()
        .find(|row| row.change.path == path)
    else {
        return false;
    };
    if let Some(view) = engine.views.get_mut(root) {
        view.folds.remove(path);
    }
    engine.keep(root, [(row.change.path, row.change.after)], now_ms);
    true
}

/// Keeps hunk `hunk` of the file at `path` of `root`'s list at `now_ms`, as Zed keeps an
/// agent's edits in a range: the hunk's new text folds into what the file is diffed from, kept
/// in `store`, so it leaves the diff and the file's other hunks stay. Keeping its last hunk
/// keeps the whole file (`keep_file`). Returns whether it was listed.
pub fn keep_hunk(
    engine: &mut Engine,
    store: &Store,
    clock: &dyn Clock,
    (root, path): (&Path, &str),
    hunk: usize,
    now_ms: u64,
) -> Result<bool, ActionError> {
    let Some(row) = list(engine, root)
        .into_iter()
        .find(|row| row.change.path == path)
    else {
        return Ok(false);
    };
    let shown = show(store, &row.change, clock)?;
    let hunk = shown
        .diff
        .hunks
        .get(hunk)
        .ok_or(ActionError::ContextChanged)?;
    let kept = diff::keep_hunk(&shown.before_text, hunk).ok_or(ActionError::ContextChanged)?;
    if kept == shown.after_text {
        return Ok(keep_file(engine, root, path, now_ms));
    }
    let entry = Entry {
        stamp: Stamp {
            bytes: kept.len() as u64,
            modified_ns: 0,
        },
        content: Content::Text(store.put(kept.as_bytes())?),
    };
    engine.keep_part(root, path, Some(entry), now_ms);
    Ok(true)
}

/// Lock files that change wholesale when a dependency does; they start collapsed.
const LOCK_FILES: [&str; 10] = [
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    "composer.lock",
    "go.sum",
];

/// Endings of generated files; they start collapsed.
const GENERATED_ENDINGS: [&str; 3] = [".min.js", ".min.css", ".map"];

/// Folders whose files a build writes; they start collapsed.
const GENERATED_FOLDERS: [&str; 2] = ["dist", "build"];

/// More lines than this added and removed start collapsed.
pub const LARGE_DIFF: usize = 400;

/// Whether `path` is a lock file or a generated one (`LOCK_FILES`, `GENERATED_ENDINGS`,
/// `GENERATED_FOLDERS`).
fn generated(path: &str) -> bool {
    let mut names: Vec<&str> = path.split('/').collect();
    let name = names.pop().unwrap_or(path);
    LOCK_FILES.contains(&name)
        || GENERATED_ENDINGS
            .iter()
            .any(|ending| name.ends_with(ending))
        || names
            .iter()
            .any(|folder| GENERATED_FOLDERS.contains(folder))
}

/// Whether a file starts collapsed as noise: a large diff (more than `LARGE_DIFF` lines added
/// and removed, `counts`), a lock or generated file, a deleted file, or one without diff lines
/// (`lined` false: a picture, a video, a sound or another binary).
pub fn starts_folded(row: &FileRow, counts: Option<(usize, usize)>, lined: bool) -> bool {
    let large = counts.is_some_and(|(added, removed)| added + removed > LARGE_DIFF);
    large || !lined || row.status == Status::Deleted || generated(&row.change.path)
}

/// Whether `row` shows collapsed in `view`: as the person chose, else as it starts (`noisy`,
/// from `starts_folded`).
pub fn folded(view: Option<&ViewState>, row: &FileRow, noisy: bool) -> bool {
    let chosen = view.and_then(|view| view.folds.get(&row.change.path).copied());
    chosen.unwrap_or(noisy)
}

/// The path of the file in view in `root`'s tab among `rows`: the one made while it is
/// listed, else the first file.
pub fn shown_selection(engine: &Engine, root: &Path, rows: &[FileRow]) -> Option<String> {
    let made = engine
        .views
        .get(root)
        .and_then(|view| view.selected.as_deref());
    let listed = |path: &str| rows.iter().any(|row| row.change.path == path);
    match made {
        Some(made) if listed(made) => Some(made.to_owned()),
        _ => rows.first().map(|row| row.change.path.clone()),
    }
}

/// The listed file in view in the tab of `root`.
pub fn selected_row(engine: &Engine, root: &Path, rows: &[FileRow]) -> Option<FileRow> {
    let shown = shown_selection(engine, root, rows)?;
    rows.iter().find(|row| row.change.path == shown).cloned()
}

/// The index of the file `step` files after `at` (before it when negative), staying at the
/// ends; the first file when none is in view.
pub fn stepped_file(count: usize, at: Option<usize>, step: i64) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match at {
        None => 0,
        Some(at) => (at as i64 + step).clamp(0, last as i64) as usize,
    })
}

/// The file to bring into view once `path` leaves the list (reverted): the one after it, else
/// the one before.
pub fn next_after(rows: &[FileRow], path: &str) -> Option<String> {
    let at = rows.iter().position(|row| row.change.path == path)?;
    rows.get(at + 1)
        .or_else(|| at.checked_sub(1).and_then(|before| rows.get(before)))
        .map(|row| row.change.path.clone())
}

/// Text pairs of every root's listed files not yet counted.
pub fn uncounted(engine: &Engine) -> Vec<Pair> {
    let wanted: BTreeSet<Pair> = engine
        .roots
        .values()
        .flat_map(unkept)
        .filter_map(|file| engine.counts.lacks(&file.change))
        .collect();
    wanted.into_iter().collect()
}

/// Lines added and removed between two kept texts.
pub fn count_lines(
    store: &Store,
    pair: &Pair,
    clock: &dyn Clock,
) -> std::io::Result<(usize, usize)> {
    let before = read_text(store, pair.0.as_ref())?;
    let after = read_text(store, pair.1.as_ref())?;
    Ok(file_diff(&before, &after, clock).counts())
}

fn read_text(store: &Store, hash: Option<&Hash>) -> std::io::Result<String> {
    match hash {
        None => Ok(String::new()),
        Some(hash) => Ok(String::from_utf8_lossy(&store.read(hash)?).into_owned()),
    }
}

fn file_diff(before: &str, after: &str, clock: &dyn Clock) -> FileDiff {
    let deadline = Deadline {
        clock,
        until_ms: clock.now_ms() + diff::DEADLINE_MS,
    };
    diff::diff(before, after, &deadline)
}

/// A file's change as the tab shows it.
#[derive(Debug, Default)]
pub struct Shown {
    pub diff: FileDiff,
    /// The texts on each side, empty where there is none, for syntax colours.
    pub before_text: String,
    pub after_text: String,
}

/// The diff of `change`; no hunks for a file without text on both sides.
pub fn show(store: &Store, change: &FileChange, clock: &dyn Clock) -> std::io::Result<Shown> {
    let Some(pair) = change.text_pair() else {
        return Ok(Shown::default());
    };
    let before_text = read_text(store, pair.0.as_ref())?;
    let after_text = read_text(store, pair.1.as_ref())?;
    Ok(Shown {
        diff: file_diff(&before_text, &after_text, clock),
        before_text,
        after_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::scan::Stamp;
    use crate::changes::snapshot::Taken;
    use crate::changes::turns::Signal;
    use crate::terminal::activity::Activity;
    use std::path::PathBuf;

    const ROOT: &str = "/p";

    fn entry(text: &str) -> Entry {
        Entry {
            stamp: Stamp {
                bytes: text.len() as u64,
                modified_ns: 0,
            },
            content: Content::Text(Hash::of(text.as_bytes())),
        }
    }

    fn taken(taken_ms: u64, files: &[(&str, &str)]) -> Taken {
        let files = files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), entry(text)))
            .collect();
        Taken {
            manifest: Manifest { taken_ms, files },
            rules: crate::changes::ignore::Rules::default(),
            unreadable: 0,
            partial: false,
        }
    }

    fn answer(engine: &mut Engine, taken_ms: u64, files: &[(&str, &str)]) {
        while let Some((request, _)) = engine.next_request() {
            engine.snapshot_taken(&request, taken(taken_ms, files));
        }
    }

    /// Two terminals in one folder, shown in a Changes tab.
    fn two_terminals() -> Engine {
        let mut engine = Engine::default();
        for id in ["a", "b"] {
            engine.add_terminal(id, Path::new(ROOT), id, || None);
        }
        answer(&mut engine, 10, &[("one.txt", "1"), ("two.txt", "2")]);
        engine.open_view(Path::new(ROOT), || None);
        answer(&mut engine, 11, &[("one.txt", "1"), ("two.txt", "2")]);
        engine
    }

    /// A turn of terminal `id` from `start` to `end` that leaves the folder as `files`.
    fn turn(engine: &mut Engine, id: &str, (start, end): (u64, u64), files: &[(&str, &str)]) {
        engine.signal(id, Signal::Hook(Activity::Working), start);
        engine.signal(id, Signal::Hook(Activity::Finished), end);
        answer(engine, end + 10, files);
    }

    fn listed(engine: &Engine) -> Vec<String> {
        list(engine, Path::new(ROOT))
            .iter()
            .map(|row| match row.contested {
                true => format!("{} !", row.change.path),
                false => row.change.path.clone(),
            })
            .collect()
    }

    #[test]
    fn files_are_listed_flat_newest_first_and_keep_their_place() {
        let mut engine = two_terminals();
        engine.signal("a", Signal::Hook(Activity::Working), 100);
        engine.signal("b", Signal::Entered, 110);
        engine.signal("a", Signal::Hook(Activity::Finished), 200);
        engine.signal("b", Signal::Unhooked(Activity::Finished), 210);
        answer(&mut engine, 220, &[("one.txt", "1!"), ("two.txt", "2")]);
        // Both turns spanned the change, at once: listed once, flagged.
        assert_eq!(listed(&engine), ["one.txt !"]);
        turn(
            &mut engine,
            "a",
            (300, 400),
            &[("one.txt", "1!"), ("two.txt", "2!")],
        );
        // A newly changed file comes in at the top.
        assert_eq!(listed(&engine), ["two.txt", "one.txt !"]);
        // One changed again keeps its place.
        turn(
            &mut engine,
            "b",
            (500, 600),
            &[("one.txt", "1!!"), ("two.txt", "2!")],
        );
        assert_eq!(listed(&engine), ["two.txt", "one.txt !"]);
    }

    #[test]
    fn a_file_the_hook_reported_is_not_flagged_for_another_terminal() {
        let mut engine = two_terminals();
        engine.signal("a", Signal::Hook(Activity::Working), 100);
        engine.signal("b", Signal::Entered, 110);
        engine.signal("a", Signal::Edited(PathBuf::from("/p/one.txt")), 150);
        engine.signal("a", Signal::Hook(Activity::Finished), 200);
        engine.signal("b", Signal::Unhooked(Activity::Finished), 210);
        answer(&mut engine, 220, &[("one.txt", "1!"), ("two.txt", "2!")]);
        // Changed by turns that started together: by path.
        assert_eq!(listed(&engine), ["one.txt", "two.txt !"]);
    }

    #[test]
    fn a_change_made_while_no_turn_ran_is_left_out() {
        let mut engine = two_terminals();
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2")],
        );
        engine.touched(&[PathBuf::from("/p/two.txt")], 5_000);
        engine.tick(7_000);
        answer(
            &mut engine,
            7_001,
            &[("one.txt", "1!"), ("two.txt", "2?"), ("shot.png", "p")],
        );
        assert_eq!(listed(&engine), ["one.txt"]);
    }

    /// The rows of `paths` as listed now.
    fn rows_of(engine: &Engine, paths: &[&str]) -> Vec<FileRow> {
        list(engine, Path::new(ROOT))
            .into_iter()
            .filter(|row| paths.contains(&row.change.path.as_str()))
            .collect()
    }

    /// Keeps every listed file whole, one by one, at `now_ms`.
    fn keep_every(engine: &mut Engine, root: &Path, now_ms: u64) {
        for row in list(engine, root) {
            assert!(keep_file(engine, root, &row.change.path, now_ms));
        }
    }

    /// Keeps just `paths` whole as listed now.
    fn keep_paths(engine: &mut Engine, paths: &[&str], now_ms: u64) {
        let entries = rows_of(engine, paths)
            .into_iter()
            .map(|row| (row.change.path, row.change.after));
        engine.keep(Path::new(ROOT), entries, now_ms);
    }

    #[test]
    fn keeping_every_file_empties_the_list_and_a_file_returns_only_once_a_terminal_changes_it() {
        let mut engine = two_terminals();
        let root = Path::new(ROOT);
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2!")],
        );
        // The person expanded one; keeping forgets it.
        engine
            .views
            .get_mut(root)
            .expect("view")
            .folds
            .insert("one.txt".into(), true);
        assert_eq!(waiting(&engine, root), 2);
        keep_every(&mut engine, root, 300);
        assert!(listed(&engine).is_empty());
        assert_eq!(waiting(&engine, root), 0);
        assert!(engine.views[root].folds.is_empty());
        // A new snapshot, or opening the tab again, keeps it empty.
        engine.tick(1_000);
        answer(&mut engine, 1_001, &[("one.txt", "1!"), ("two.txt", "2!")]);
        engine.open_view(root, || None);
        answer(&mut engine, 1_010, &[("one.txt", "1!"), ("two.txt", "2!")]);
        assert!(listed(&engine).is_empty());
        // The person's own edit, or another program's, made while no turn ran: still empty.
        engine.touched(&[PathBuf::from("/p/one.txt")], 3_000);
        engine.tick(5_000);
        answer(&mut engine, 5_001, &[("one.txt", "1?"), ("two.txt", "2!")]);
        assert!(listed(&engine).is_empty());
        assert_eq!(waiting(&engine, root), 0);
        // A terminal changes it again: only it lists, expanded, diffed from the kept version.
        turn(
            &mut engine,
            "b",
            (6_000, 6_100),
            &[("one.txt", "1!!"), ("two.txt", "2!")],
        );
        assert_eq!(listed(&engine), ["one.txt"]);
        let rows = list(&engine, root);
        assert_eq!(rows[0].change.before, Some(entry("1!")));
        assert_eq!(rows[0].change.after, Some(entry("1!!")));
        assert!(!folded(engine.views.get(root), &rows[0], false));
        assert_eq!(waiting(&engine, root), 1);
    }

    #[test]
    fn keeping_while_a_turn_runs_lists_only_what_it_changes_after() {
        let mut engine = two_terminals();
        let root = Path::new(ROOT);
        engine.signal("a", Signal::Hook(Activity::Working), 100);
        answer(&mut engine, 150, &[("one.txt", "1"), ("two.txt", "2")]);
        engine.refresh(root);
        answer(&mut engine, 210, &[("one.txt", "1!"), ("two.txt", "2")]);
        assert_eq!(listed(&engine), ["one.txt"]);
        keep_every(&mut engine, root, 300);
        assert!(listed(&engine).is_empty());
        engine.signal("a", Signal::Hook(Activity::Finished), 400);
        answer(&mut engine, 410, &[("one.txt", "1!!"), ("two.txt", "2")]);
        let rows = list(&engine, root);
        assert_eq!(listed(&engine), ["one.txt"]);
        assert_eq!(rows[0].change.before, Some(entry("1!")));
    }

    /// A row of `path`, `status`.
    fn row_at(path: &str, status: Status) -> FileRow {
        let side = || Some(entry("x"));
        let (before, after) = match status {
            Status::Added => (None, side()),
            Status::Modified => (side(), side()),
            Status::Deleted => (side(), None),
        };
        FileRow {
            change: FileChange {
                path: path.into(),
                before,
                after,
            },
            status,
            counts: None,
            contested: false,
            entered_ms: 0,
        }
    }

    #[test]
    fn noisy_files_start_collapsed() {
        let plain = row_at("src/main.rs", Status::Modified);
        assert!(!starts_folded(&plain, Some((200, 200)), true));
        // More than 400 lines added and removed.
        assert!(starts_folded(&plain, Some((201, 200)), true));
        // Lock and generated files, anywhere.
        for path in [
            "package-lock.json",
            "web/yarn.lock",
            "pnpm-lock.yaml",
            "bun.lock",
            "bun.lockb",
            "Cargo.lock",
            "Gemfile.lock",
            "poetry.lock",
            "composer.lock",
            "go.sum",
            "app.min.js",
            "site/app.min.css",
            "app.js.map",
            "dist/index.js",
            "web/build/out/page.html",
        ] {
            assert!(
                starts_folded(&row_at(path, Status::Modified), Some((1, 0)), true),
                "{path}"
            );
        }
        for path in ["build.rs", "src/dist.rs", "distance/notes.md", "lock.md"] {
            assert!(
                !starts_folded(&row_at(path, Status::Modified), Some((1, 0)), true),
                "{path}"
            );
        }
        // Deleted, or without diff lines (a picture).
        assert!(starts_folded(
            &row_at("old.md", Status::Deleted),
            Some((0, 3)),
            true
        ));
        assert!(starts_folded(
            &row_at("shot.png", Status::Added),
            None,
            false
        ));
    }

    #[test]
    fn a_choice_to_expand_or_collapse_beats_how_a_file_starts() {
        let mut view = ViewState::default();
        let row = row_at("package-lock.json", Status::Modified);
        assert!(folded(Some(&view), &row, true));
        assert!(!folded(None, &row, false));
        view.folds.insert(row.change.path.clone(), false);
        assert!(!folded(Some(&view), &row, true));
        view.folds.insert(row.change.path.clone(), true);
        assert!(folded(Some(&view), &row, false));
    }

    #[test]
    fn a_file_changed_back_as_it_was_kept_is_not_listed() {
        let mut engine = two_terminals();
        let root = Path::new(ROOT);
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2")],
        );
        keep_every(&mut engine, root, 300);
        turn(
            &mut engine,
            "a",
            (400, 500),
            &[("one.txt", "1?"), ("two.txt", "2")],
        );
        turn(
            &mut engine,
            "a",
            (600, 700),
            &[("one.txt", "1!"), ("two.txt", "2")],
        );
        assert!(listed(&engine).is_empty());
    }

    /// The test root with `texts` kept in `store`, so diffs read them.
    fn stored(store: &Store, texts: &[&str]) {
        for text in texts {
            store.put(text.as_bytes()).expect("put");
        }
    }

    #[test]
    fn keeping_a_hunk_moves_its_text_into_the_baseline_and_leaves_the_file_s_other_hunks() {
        let data = tempfile::tempdir().expect("store");
        let store = Store::at(data.path());
        let clock = crate::changes::testing::FixedClock::at(0);
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old
            .replacen("2\n", "two\n", 1)
            .replacen("19\n", "nineteen\n", 1);
        stored(&store, &[&old, &new]);
        let mut engine = Engine::default();
        let root = Path::new(ROOT);
        engine.add_terminal("a", root, "a", || None);
        answer(&mut engine, 10, &[("a.txt", old.as_str()), ("b.txt", "b")]);
        engine.open_view(root, || None);
        answer(&mut engine, 11, &[("a.txt", old.as_str()), ("b.txt", "b")]);
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("a.txt", new.as_str()), ("b.txt", "b!")],
        );
        stored(&store, &["b", "b!"]);
        let hunks = |engine: &Engine| {
            let row = &rows_of(engine, &["a.txt"])[0];
            show(&store, &row.change, &clock).expect("shown").diff.hunks
        };
        assert_eq!(hunks(&engine).len(), 2);
        let second = hunks(&engine)[1].header();
        // Keeping the first: only the second is left, and the file stays listed.
        let kept = keep_hunk(&mut engine, &store, &clock, (root, "a.txt"), 0, 300);
        assert!(kept.expect("kept"));
        assert_eq!(listed(&engine), ["a.txt", "b.txt"]);
        let left = hunks(&engine);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].header(), second);
        // It stays so across a restart.
        let record = view_record(&engine, 400);
        assert_eq!(record.kept["a.txt"].since_ms, 0);
        let later = restarted(
            record,
            "a",
            "a",
            &[("a.txt", new.as_str()), ("b.txt", "b!")],
        );
        assert_eq!(listed(&later), ["a.txt", "b.txt"]);
        // Keeping the last keeps the whole file: it leaves, and the person's own edit after
        // doesn't bring it back.
        let kept = keep_hunk(&mut engine, &store, &clock, (root, "a.txt"), 0, 500);
        assert!(kept.expect("kept"));
        assert_eq!(listed(&engine), ["b.txt"]);
        engine.touched(&[PathBuf::from("/p/a.txt")], 5_000);
        engine.tick(7_000);
        answer(&mut engine, 7_001, &[("a.txt", "mine\n"), ("b.txt", "b!")]);
        assert_eq!(listed(&engine), ["b.txt"]);
        // A file not listed keeps nothing.
        let none = keep_hunk(&mut engine, &store, &clock, (root, "a.txt"), 0, 600);
        assert!(!none.expect("nothing"));
        // Keeping a file whole.
        assert!(keep_file(&mut engine, root, "b.txt", 700));
        assert!(listed(&engine).is_empty());
        assert!(!keep_file(&mut engine, root, "b.txt", 800));
    }

    #[test]
    fn the_file_in_view_falls_back_to_the_first_and_moves_on_once_it_leaves() {
        let mut engine = two_terminals();
        let root = Path::new(ROOT);
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2!")],
        );
        let rows = list(&engine, root);
        assert_eq!(
            shown_selection(&engine, root, &rows).as_deref(),
            Some("one.txt")
        );
        assert_eq!(stepped_file(2, Some(0), 1), Some(1));
        assert_eq!(stepped_file(2, Some(1), 1), Some(1));
        assert_eq!(stepped_file(2, Some(1), -5), Some(0));
        assert_eq!(stepped_file(2, None, -1), Some(0));
        assert_eq!(stepped_file(0, None, 1), None);
        engine.views.get_mut(root).expect("view").selected = Some("two.txt".to_owned());
        let shown = selected_row(&engine, root, &rows).expect("in view");
        assert_eq!(shown.change.path, "two.txt");
        // Reverting the file in view brings the next into view, else the one before.
        assert_eq!(next_after(&rows, "one.txt").as_deref(), Some("two.txt"));
        assert_eq!(next_after(&rows, "two.txt").as_deref(), Some("one.txt"));
        assert_eq!(next_after(&rows[..1], "one.txt"), None);
        // Once it is no longer listed, the first file is in view again.
        engine.keep(root, [("two.txt".to_owned(), Some(entry("2!")))], 300);
        let rows = list(&engine, root);
        assert_eq!(
            shown_selection(&engine, root, &rows).as_deref(),
            Some("one.txt")
        );
    }

    /// The engine of a run after `record` was kept: terminal `id` credited as `credit`, the
    /// folder now `files`, the tab opened.
    fn restarted(record: Record, id: &str, credit: &str, files: &[(&str, &str)]) -> Engine {
        let mut engine = Engine::default();
        let root = Path::new(ROOT);
        engine.add_terminal(id, root, credit, || None);
        engine.restore(root, record);
        answer(&mut engine, 10_000, files);
        engine.open_view(root, || None);
        answer(&mut engine, 10_001, files);
        engine
    }

    #[test]
    fn a_restart_keeps_the_list_its_baselines_credit_and_kept_marks() {
        let mut engine = two_terminals();
        let root = Path::new(ROOT);
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2!")],
        );
        keep_paths(&mut engine, &["two.txt"], 300);
        let record = record(&engine.roots[root], 400);
        let one = &record.listed["one.txt"];
        assert_eq!(record.listed.len(), 1, "a kept file is not listed");
        assert_eq!(one.baseline, Some(entry("1")));
        assert_eq!(one.terminals, BTreeSet::from(["a".to_owned()]));
        assert_eq!((one.entered_ms, one.changed_ms), (100, 200));
        assert_eq!(record.kept["two.txt"].entry, Some(entry("2!")));
        // A new run: the file is listed from its baseline, the kept one is not.
        let files = [("one.txt", "1!"), ("two.txt", "2!")];
        let mut later = restarted(record.clone(), "t9", "b", &files);
        assert_eq!(listed(&later), ["one.txt"]);
        let rows = list(&later, root);
        assert_eq!(
            (rows[0].change.before.clone(), rows[0].entered_ms),
            (Some(entry("1")), 100)
        );
        assert_eq!(later.roots[root].kept["two.txt"].at_ms, 300);
        assert_eq!(waiting(&later, root), 1);
        // Kept again unchanged, it is the same record.
        assert_eq!(view_record(&later, 10_002), record);
        // A kept file a terminal changes again lists from its kept version.
        turn(
            &mut later,
            "t9",
            (11_000, 11_100),
            &[("one.txt", "1!"), ("two.txt", "2!!")],
        );
        assert_eq!(listed(&later), ["two.txt", "one.txt"]);
        let two = &rows_of(&later, &["two.txt"])[0];
        assert_eq!(two.change.before, Some(entry("2!")));
        // Keeping drops both from the next record, the carried one too.
        keep_every(&mut later, root, 12_000);
        let kept = view_record(&later, 12_001);
        assert!(kept.listed.is_empty());
        assert_eq!(kept.kept["one.txt"].entry, Some(entry("1!")));
        assert_eq!(waiting(&later, root), 0);
    }

    fn view_record(engine: &Engine, now_ms: u64) -> Record {
        record(&engine.roots[Path::new(ROOT)], now_ms)
    }

    /// One file a turn of terminal "a" changed, kept as a record.
    fn one_changed() -> Record {
        let mut engine = two_terminals();
        turn(
            &mut engine,
            "a",
            (100, 200),
            &[("one.txt", "1!"), ("two.txt", "2")],
        );
        view_record(&engine, 300)
    }

    #[test]
    fn a_file_put_back_as_it_was_while_the_app_was_closed_drops_off() {
        let files = [("one.txt", "1"), ("two.txt", "2")];
        let later = restarted(one_changed(), "a", "a", &files);
        assert!(listed(&later).is_empty());
        assert_eq!(waiting(&later, Path::new(ROOT)), 0);
        assert!(view_record(&later, 10_002).listed.is_empty());
    }

    #[test]
    fn a_file_changed_while_the_app_was_closed_stays_listed_with_the_credit_it_had() {
        let files = [("one.txt", "1?"), ("two.txt", "2")];
        let later = restarted(one_changed(), "a", "a", &files);
        let rows = list(&later, Path::new(ROOT));
        assert_eq!(listed(&later), ["one.txt"]);
        assert_eq!(rows[0].change.before, Some(entry("1")));
        assert_eq!(rows[0].change.after, Some(entry("1?")));
        let kept = view_record(&later, 10_002);
        // No new credit, and no new time, for an edit made while the app was closed.
        assert_eq!(kept.listed, one_changed().listed);
    }

    #[test]
    fn credit_follows_the_terminal_file_not_the_id_a_run_gives_it() {
        let files = [("one.txt", "1!"), ("two.txt", "2")];
        // The same terminal file comes back under a new id.
        let mut later = restarted(one_changed(), "t42", "a", &files);
        turn(
            &mut later,
            "t42",
            (11_000, 11_100),
            &[("one.txt", "1!!"), ("two.txt", "2")],
        );
        let kept = view_record(&later, 11_200);
        let one = &kept.listed["one.txt"];
        assert_eq!(one.terminals, BTreeSet::from(["a".to_owned()]));
        assert_eq!(one.baseline, Some(entry("1")));
        assert_eq!((one.entered_ms, one.changed_ms), (100, 11_100));
        // Another terminal file adds its own credit.
        later.add_terminal("t43", Path::new(ROOT), "b", || None);
        turn(
            &mut later,
            "t43",
            (12_000, 12_100),
            &[("one.txt", "1!!!"), ("two.txt", "2")],
        );
        let kept = view_record(&later, 12_200);
        assert_eq!(
            kept.listed["one.txt"].terminals,
            BTreeSet::from(["a".to_owned(), "b".to_owned()])
        );
    }

    #[test]
    fn before_the_first_snapshot_the_record_and_count_are_what_the_last_run_left() {
        let mut engine = Engine::default();
        let root = Path::new(ROOT);
        engine.watch_root(root, || None);
        engine.restore(root, one_changed());
        assert_eq!(waiting(&engine, root), 1);
        assert_eq!(view_record(&engine, 400), one_changed());
        assert_eq!(engine.busiest_root(), Some(root));
        assert!(referenced(&engine).contains(&Hash::of(b"1")));
        // Past the store's 14 days it is dropped.
        let late = 200 + crate::changes::store::KEEP_MS + 1;
        assert!(view_record(&engine, late).listed.is_empty());
    }

    #[test]
    fn a_deleted_text_file_shows_every_line_it_had_as_removed() {
        let folder = tempfile::tempdir().expect("store");
        let store = Store::at(folder.path());
        let hash = store.put(b"one\ntwo\nthree\n").expect("kept");
        let before = Entry {
            content: Content::Text(hash),
            ..entry("one\ntwo\nthree\n")
        };
        let change = FileChange {
            path: "gone.md".into(),
            before: Some(before),
            after: None,
        };
        assert_eq!(change.status(), Status::Deleted);
        let clock = crate::changes::testing::FixedClock::at(0);
        let shown = show(&store, &change, &clock).expect("shown");
        let lines: Vec<_> = shown
            .diff
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .collect();
        assert_eq!(lines.len(), 3);
        assert!(lines
            .iter()
            .all(|line| line.kind == diff::LineKind::Removed));
        assert_eq!(lines[2].shown(), "three");
        assert_eq!(shown.before_text, "one\ntwo\nthree\n");
    }

    #[test]
    fn statuses_follow_each_side_and_only_text_has_lines() {
        let change = |before, after| FileChange {
            path: "x".into(),
            before,
            after,
        };
        let text = || Some(entry("a"));
        let picture = || {
            Some(Entry {
                content: Content::Binary(Hash::of(b"b")),
                ..entry("b")
            })
        };
        assert_eq!(change(None, text()).status(), Status::Added);
        assert_eq!(change(text(), None).status(), Status::Deleted);
        assert_eq!(change(text(), text()).status(), Status::Modified);
        assert!(change(None, text()).is_text());
        assert_eq!(change(text(), picture()).status(), Status::Modified);
        assert!(!change(None, picture()).is_text());
    }
}
