//! Turns and the folders they change. A turn of a terminal starts when Claude Code's hook
//! reports work after a pause, or, for programs without hooks, when the person presses Return;
//! it ends on the hook's stop, or once the activity settles or the folder stays quiet. Each turn
//! has a snapshot of its folder at both ends; the engine asks for them (`Request`) and the
//! service takes them off the UI thread.

use crate::changes::actions::Undone;
use crate::changes::git::{Repository, GIT_MARK};
use crate::changes::ignore::{Kind, Rules};
use crate::changes::record::{Listed, Record};
use crate::changes::scan;
use crate::changes::snapshot::{Entry, Manifest, Taken};
use crate::terminal::activity::Activity;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A turn without hooks ends after this long with no file changes in its folder.
pub const QUIET_MS: u64 = 3_000;
/// Changes seen this soon after a turn ended still count as the turn's.
const GRACE_MS: u64 = 2_000;
/// Changes made outside any turn are read this long after they stop.
const REFRESH_AFTER_MS: u64 = 1_000;

/// A terminal by its terminal file's id, for this run of the app.
pub type TerminalId = String;

/// What a terminal reports that matters to its turns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signal {
    /// The person pressed Return in a terminal whose program reports through no hook.
    Entered,
    /// Northroom's hook reported an activity.
    Hook(Activity),
    /// A program without hooks changed its activity, read from its title or notifications.
    Unhooked(Activity),
    /// The hook reported that the agent edited this file.
    Edited(PathBuf),
}

/// Sends a terminal's signals to the changes. Empty for a terminal nobody follows, as in tests.
#[derive(Clone, Default)]
pub struct TurnTap(Option<Arc<dyn Fn(Signal) + Send + Sync>>);

impl TurnTap {
    pub fn new(send: impl Fn(Signal) + Send + Sync + 'static) -> Self {
        Self(Some(Arc::new(send)))
    }

    pub fn send(&self, signal: Signal) {
        if let Some(send) = &self.0 {
            send(signal);
        }
    }
}

/// A terminal's turn by its number, from 1 in each run of the app.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TurnKey {
    pub terminal: TerminalId,
    pub number: u32,
}

/// How a turn learns that it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ending {
    /// The hook's stop.
    Hooked,
    /// The activity settling, or a quiet folder while the program is not working.
    Unhooked { working: bool },
}

#[derive(Clone, Debug)]
pub struct Turn {
    pub key: TurnKey,
    /// Its terminal as credit names it across runs: the terminal file's path relative to the
    /// root (`record`).
    pub credit: String,
    pub started_ms: u64,
    pub ended_ms: Option<u64>,
    /// The folder at the start and end, once taken.
    pub before: Option<Arc<Manifest>>,
    pub after: Option<Arc<Manifest>>,
    /// Files the hook reported this turn edited.
    pub reported: BTreeSet<String>,
    ending: Ending,
}

impl Turn {
    /// Whether this turn and `other` ran at the same time.
    pub fn overlaps(&self, other: &Turn) -> bool {
        let end = |turn: &Turn| turn.ended_ms.unwrap_or(u64::MAX);
        self.started_ms <= end(other) && other.started_ms <= end(self)
    }

    /// Whether the turn was running at `at_ms`, or ended just before it.
    fn spans(&self, at_ms: u64) -> bool {
        let end = self
            .ended_ms
            .map_or(u64::MAX, |ended| ended.saturating_add(GRACE_MS));
        self.started_ms <= at_ms && at_ms <= end
    }
}

/// Why a snapshot is wanted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// The folder as the app began watching it.
    Base,
    TurnStart(TurnKey),
    TurnEnd(TurnKey),
    /// The folder now, after changes outside any turn or for the view.
    Refresh,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub root: PathBuf,
    pub purpose: Purpose,
}

/// What the person kept of a file: the version it is diffed from now (`None` for a deleted
/// file), when it was kept, and from when turns count towards listing it. Keeping the whole file
/// takes it off the list until a terminal changes it after that (`since_ms` is `at_ms`); keeping
/// one hunk folds that hunk into the version and leaves `since_ms` where it was, so the file's
/// other hunks stay listed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Kept {
    pub entry: Option<Entry>,
    pub at_ms: u64,
    /// Turns that ended before this do not list the file.
    #[serde(default)]
    pub since_ms: u64,
}

/// A watched folder.
pub struct Root {
    pub path: PathBuf,
    /// The latest snapshot of this run.
    pub latest: Option<Arc<Manifest>>,
    /// The last kept snapshot, only to read less in the first one.
    cache: Option<Arc<Manifest>>,
    /// The folder as this run began watching it.
    pub session_base: Option<Arc<Manifest>>,
    pub turns: Vec<Turn>,
    /// What the person kept of each file, by path.
    pub kept: BTreeMap<String, Kept>,
    /// Files listed when an earlier run quit, by path, from the root's record (`carried`).
    pub carried: BTreeMap<String, Listed>,
    /// The scope its Changes tab shows, as last chosen this run (`view::shown_scope`).
    pub scope: crate::changes::view::Scope,
    /// What git says of it, once read; `None` outside a repository or without git.
    pub git: Option<Arc<Repository>>,
    /// Git is to be read again: a snapshot was taken or the repository changed.
    git_due: bool,
    pub(crate) rules: Rules,
    last_change_ms: u64,
    refresh_due: bool,
    running: BTreeSet<TerminalId>,
}

impl Root {
    fn new(path: PathBuf, cache: Option<Manifest>) -> Self {
        Self {
            path,
            latest: None,
            cache: cache.map(Arc::new),
            session_base: None,
            turns: Vec::new(),
            kept: BTreeMap::new(),
            carried: BTreeMap::new(),
            scope: crate::changes::view::Scope::New,
            git: None,
            git_due: true,
            rules: Rules::default(),
            last_change_ms: 0,
            refresh_due: false,
            running: BTreeSet::new(),
        }
    }

    pub fn turn(&self, key: &TurnKey) -> Option<&Turn> {
        self.turns.iter().find(|turn| &turn.key == key)
    }

    /// What an earlier run listed of `path`, unless it was kept whole since a turn last changed
    /// it.
    pub fn carried(&self, path: &str) -> Option<&Listed> {
        let listed = self.carried.get(path)?;
        let kept_since = self
            .kept
            .get(path)
            .is_some_and(|kept| kept.since_ms >= listed.changed_ms);
        (!kept_since).then_some(listed)
    }

    fn turn_mut(&mut self, key: &TurnKey) -> Option<&mut Turn> {
        self.turns.iter_mut().find(|turn| &turn.key == key)
    }

    /// Whether a turn runs at `at_ms`.
    fn in_turn(&self, at_ms: u64) -> bool {
        self.turns.iter().any(|turn| turn.spans(at_ms))
    }

    /// Whether the latest snapshot still shows the folder: nothing changed since it.
    fn settled(&self) -> bool {
        self.latest
            .as_ref()
            .is_some_and(|latest| self.last_change_ms <= latest.taken_ms)
    }

    /// Whether `path` (relative) is one the changes leave out.
    fn leaves_out(&self, path: &str) -> bool {
        leaves_out(&self.rules, path)
    }
}

/// Whether the changes leave out `path` (relative): skipped names and what `rules` ignore.
pub fn leaves_out(rules: &Rules, path: &str) -> bool {
    scan::skips(path) || rules.ignores(path, Kind::File)
}

/// What a settling activity does to a running turn.
enum Next {
    Keep,
    Close,
    Open,
}

/// A terminal the changes follow.
pub struct Followed {
    pub root: PathBuf,
    /// Its terminal file's path relative to the root, which credit goes by (`Turn::credit`).
    pub credit: String,
    /// Its running turn's number.
    open: Option<u32>,
    turns: u32,
}

/// Every watched folder and followed terminal, and the snapshots still to take.
#[derive(Default)]
pub struct Engine {
    pub roots: BTreeMap<PathBuf, Root>,
    pub terminals: BTreeMap<TerminalId, Followed>,
    requests: VecDeque<Request>,
    /// How each root a Changes tab shows is set, by root; a root is watched while it has one.
    pub views: BTreeMap<PathBuf, crate::changes::view::ViewState>,
    /// Lines added and removed per changed text, counted off the UI thread.
    pub counts: crate::changes::view::Counts,
    /// What the last Undo did, file by file, which ⌘Z puts back (`actions::restore`); empty
    /// once put back or after a Keep.
    pub undone: Vec<Undone>,
    revision: u64,
}

impl Engine {
    /// Goes up whenever what the view shows may have changed.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn touch(&mut self) {
        self.revision += 1;
    }

    /// Watches `root`, reading the snapshot an earlier run kept of it with `load`, unless it is
    /// watched.
    pub fn watch_root(&mut self, root: &Path, load: impl FnOnce() -> Option<Manifest>) {
        if self.roots.contains_key(root) {
            return;
        }
        self.roots
            .insert(root.to_path_buf(), Root::new(root.to_path_buf(), load()));
        self.requests.push_back(Request {
            root: root.to_path_buf(),
            purpose: Purpose::Base,
        });
        self.touch();
    }

    /// Takes up what an earlier run kept of `root` (`record`): the files it listed, and its
    /// kept marks where this run has none.
    pub fn restore(&mut self, root: &Path, record: Record) {
        let Some(watched) = self.roots.get_mut(root) else {
            return;
        };
        watched.carried = record.listed;
        for (path, kept) in record.kept {
            watched.kept.entry(path).or_insert(kept);
        }
        self.touch();
    }

    /// Follows terminal `id`, working in folder `root`, credited as `credit` (its terminal
    /// file's path relative to `root`).
    pub fn add_terminal(
        &mut self,
        id: &str,
        root: &Path,
        credit: &str,
        load: impl FnOnce() -> Option<Manifest>,
    ) {
        self.watch_root(root, load);
        if let Some(watched) = self.roots.get_mut(root) {
            watched.running.insert(id.to_owned());
        }
        self.terminals.entry(id.to_owned()).or_insert(Followed {
            root: root.to_path_buf(),
            credit: credit.to_owned(),
            open: None,
            turns: 0,
        });
        self.touch();
    }

    /// Stops following terminal `id`'s session; a running turn ends. Its turns stay listed.
    pub fn remove_terminal(&mut self, id: &str, now_ms: u64) {
        self.close_turn(id, now_ms);
        let Some(followed) = self.terminals.get(id) else {
            return;
        };
        if let Some(root) = self.roots.get_mut(&followed.root) {
            root.running.remove(id);
        }
        self.touch();
    }

    /// The folders to watch: those with a running terminal, and those a Changes tab shows.
    pub fn watched_roots(&self) -> BTreeSet<PathBuf> {
        let mut watched: BTreeSet<PathBuf> = self
            .roots
            .values()
            .filter(|root| !root.running.is_empty())
            .map(|root| root.path.clone())
            .collect();
        watched.extend(self.views.keys().cloned());
        watched
    }

    /// Applies what terminal `id` reported at `now_ms`.
    pub fn signal(&mut self, id: &str, signal: Signal, now_ms: u64) {
        match signal {
            Signal::Entered => self.open_turn(id, Ending::Unhooked { working: false }, now_ms),
            Signal::Hook(Activity::Working) => self.open_turn(id, Ending::Hooked, now_ms),
            Signal::Hook(Activity::Finished | Activity::Idle | Activity::Ended) => {
                self.close_turn(id, now_ms);
            }
            Signal::Hook(Activity::NeedsYou | Activity::Starting) => {}
            Signal::Unhooked(activity) => self.settle(id, activity, now_ms),
            Signal::Edited(path) => self.report_edit(id, &path, now_ms),
        }
    }

    /// Starts a turn of terminal `id` unless one runs; a running turn that hears from the hook
    /// waits for the hook's stop from then on.
    fn open_turn(&mut self, id: &str, ending: Ending, now_ms: u64) {
        let Some(followed) = self.terminals.get_mut(id) else {
            return;
        };
        let Some(root) = self.roots.get_mut(&followed.root) else {
            return;
        };
        if let Some(number) = followed.open {
            let key = TurnKey {
                terminal: id.to_owned(),
                number,
            };
            if let (Some(turn), Ending::Hooked) = (root.turn_mut(&key), ending) {
                turn.ending = Ending::Hooked;
            }
            return;
        }
        followed.turns += 1;
        followed.open = Some(followed.turns);
        let key = TurnKey {
            terminal: id.to_owned(),
            number: followed.turns,
        };
        let pending = self
            .requests
            .iter()
            .any(|request| request.root == root.path);
        let before = root.latest.clone().filter(|_| root.settled() && !pending);
        if before.is_none() {
            self.requests.push_back(Request {
                root: root.path.clone(),
                purpose: Purpose::TurnStart(key.clone()),
            });
        }
        root.turns.push(Turn {
            key,
            credit: followed.credit.clone(),
            started_ms: now_ms,
            ended_ms: None,
            before,
            after: None,
            reported: BTreeSet::new(),
            ending,
        });
        self.revision += 1;
    }

    /// Ends terminal `id`'s running turn and asks for its closing snapshot.
    fn close_turn(&mut self, id: &str, now_ms: u64) {
        let Some(followed) = self.terminals.get_mut(id) else {
            return;
        };
        let Some(number) = followed.open.take() else {
            return;
        };
        let key = TurnKey {
            terminal: id.to_owned(),
            number,
        };
        let Some(root) = self.roots.get_mut(&followed.root) else {
            return;
        };
        if let Some(turn) = root.turn_mut(&key) {
            turn.ended_ms = Some(now_ms);
        }
        self.requests.push_back(Request {
            root: root.path.clone(),
            purpose: Purpose::TurnEnd(key),
        });
        self.revision += 1;
    }

    /// The running turn of terminal `id`, with its folder.
    fn running_turn(&mut self, id: &str) -> Option<&mut Turn> {
        let followed = self.terminals.get(id)?;
        let key = TurnKey {
            terminal: id.to_owned(),
            number: followed.open?,
        };
        self.roots.get_mut(&followed.root)?.turn_mut(&key)
    }

    /// A program without hooks changed its activity: work keeps its turn open, and settling
    /// ends it. Work with no turn running starts one.
    fn settle(&mut self, id: &str, activity: Activity, now_ms: u64) {
        let working = activity == Activity::Working;
        let next = match self.running_turn(id) {
            Some(turn) if turn.ending == Ending::Hooked => Next::Keep,
            Some(turn) if working || activity == Activity::NeedsYou => {
                turn.ending = Ending::Unhooked { working };
                Next::Keep
            }
            Some(_) => Next::Close,
            None if working => Next::Open,
            None => Next::Keep,
        };
        match next {
            Next::Keep => {}
            Next::Close => self.close_turn(id, now_ms),
            Next::Open => self.open_turn(id, Ending::Unhooked { working }, now_ms),
        }
    }

    /// The hook says terminal `id` edited `path`: the file is the turn's alone.
    fn report_edit(&mut self, id: &str, path: &Path, now_ms: u64) {
        let Some(folder) = self.terminals.get(id).map(|followed| followed.root.clone()) else {
            return;
        };
        let Some(relative) = relative_to(&folder, path) else {
            return;
        };
        if let Some(root) = self.roots.get_mut(&folder) {
            root.last_change_ms = root.last_change_ms.max(now_ms);
        }
        if let Some(turn) = self.running_turn(id) {
            turn.reported.insert(relative);
        }
    }

    /// The watcher saw `paths` change at `now_ms`. A change while no turn ran is read once the
    /// folder is quiet; it is credited to no terminal.
    pub fn touched(&mut self, paths: &[PathBuf], now_ms: u64) {
        for path in paths {
            let Some(root) = self.root_holding_mut(path) else {
                continue;
            };
            let Some(relative) = relative_to(&root.path, path) else {
                continue;
            };
            if relative.split('/').next() == Some(GIT_MARK) {
                // A commit, checkout or staging: git is read again.
                root.git_due = true;
                continue;
            }
            if root.leaves_out(&relative) {
                continue;
            }
            root.last_change_ms = root.last_change_ms.max(now_ms);
            if !root.in_turn(now_ms) {
                root.refresh_due = true;
            }
        }
    }

    /// The deepest watched root holding `path`.
    fn root_holding_mut(&mut self, path: &Path) -> Option<&mut Root> {
        self.roots
            .values_mut()
            .filter(|root| path.starts_with(&root.path))
            .max_by_key(|root| root.path.components().count())
    }

    /// Ends turns whose folder went quiet and asks to read changes made outside turns.
    pub fn tick(&mut self, now_ms: u64) {
        let quiet: Vec<TerminalId> = self
            .terminals
            .iter()
            .filter(|(id, followed)| self.went_quiet(id, followed, now_ms))
            .map(|(id, _)| id.clone())
            .collect();
        for id in quiet {
            self.close_turn(&id, now_ms);
        }
        for root in self.roots.values_mut() {
            let quiet = now_ms.saturating_sub(root.last_change_ms) >= REFRESH_AFTER_MS;
            if root.refresh_due && quiet && !root.in_turn(now_ms) {
                root.refresh_due = false;
                self.requests.push_back(Request {
                    root: root.path.clone(),
                    purpose: Purpose::Refresh,
                });
            }
        }
    }

    /// Whether terminal `id`'s turn waits for a quiet folder and has had one long enough.
    fn went_quiet(&self, id: &str, followed: &Followed, now_ms: u64) -> bool {
        let Some(number) = followed.open else {
            return false;
        };
        let Some(root) = self.roots.get(&followed.root) else {
            return false;
        };
        let key = TurnKey {
            terminal: id.to_owned(),
            number,
        };
        let Some(turn) = root.turn(&key) else {
            return false;
        };
        let since = turn.started_ms.max(root.last_change_ms);
        turn.ending == (Ending::Unhooked { working: false })
            && now_ms.saturating_sub(since) >= QUIET_MS
    }

    /// Asks for a snapshot of `root` now, as when the view opens on it.
    pub fn refresh(&mut self, root: &Path) {
        let asked = self.requests.iter().any(|request| request.root == root);
        if self.roots.contains_key(root) && !asked {
            self.requests.push_back(Request {
                root: root.to_path_buf(),
                purpose: Purpose::Refresh,
            });
        }
    }

    /// The next snapshot to take, with the manifest it may reuse what was read from.
    pub fn next_request(&mut self) -> Option<(Request, Arc<Manifest>)> {
        let request = self.requests.pop_front()?;
        let previous = self
            .roots
            .get(&request.root)
            .and_then(|root| root.latest.clone().or_else(|| root.cache.clone()));
        Some((request, previous.unwrap_or_default()))
    }

    /// Records the snapshot taken for `request`.
    pub fn snapshot_taken(&mut self, request: &Request, taken: Taken) {
        let Some(root) = self.roots.get_mut(&request.root) else {
            return;
        };
        let manifest = Arc::new(taken.manifest);
        root.latest = Some(Arc::clone(&manifest));
        root.cache = None;
        root.rules = taken.rules;
        root.git_due = true;
        match &request.purpose {
            Purpose::Base => {
                root.session_base.get_or_insert(manifest);
            }
            Purpose::TurnStart(key) => {
                if let Some(turn) = root.turn_mut(key) {
                    turn.before = Some(manifest);
                }
            }
            Purpose::TurnEnd(key) => {
                if let Some(turn) = root.turn_mut(key) {
                    turn.after = Some(manifest);
                }
            }
            Purpose::Refresh => {}
        }
        self.revision += 1;
    }

    /// Shows `root` in a Changes tab, watching it if it is not and reading it now. A root shown
    /// before keeps how it was set and scrolled.
    pub fn open_view(&mut self, root: &Path, load: impl FnOnce() -> Option<Manifest>) {
        self.watch_root(root, load);
        self.refresh(root);
        self.views.entry(root.to_path_buf()).or_default();
        self.touch();
    }

    /// No Changes tab shows `root` any more.
    pub fn close_view(&mut self, root: &Path) {
        self.views.remove(root);
        self.touch();
    }

    /// Keeps `files` of `root` whole at `now_ms`, each at its entry now (`None` for a deleted
    /// file), so they leave the list until a terminal changes them again, diffed from that entry.
    pub fn keep(
        &mut self,
        root: &Path,
        files: impl IntoIterator<Item = (String, Option<Entry>)>,
        now_ms: u64,
    ) {
        let Some(watched) = self.roots.get_mut(root) else {
            return;
        };
        for (path, entry) in files {
            let kept = Kept {
                entry,
                at_ms: now_ms,
                since_ms: now_ms,
            };
            watched.kept.insert(path, kept);
        }
        self.undone.clear();
        self.touch();
    }

    /// Keeps part of `path` of `root` at `now_ms`: it is diffed from `entry` from now on, and
    /// stays listed for the turns that listed it.
    pub fn keep_part(&mut self, root: &Path, path: &str, entry: Option<Entry>, now_ms: u64) {
        let Some(watched) = self.roots.get_mut(root) else {
            return;
        };
        let since_ms = watched.kept.get(path).map_or(0, |kept| kept.since_ms);
        let kept = Kept {
            entry,
            at_ms: now_ms,
            since_ms,
        };
        watched.kept.insert(path.to_owned(), kept);
        self.undone.clear();
        self.touch();
    }

    /// A shown root whose git is due to be read: its path, latest snapshot and what it leaves
    /// out. Asking clears the request.
    pub fn git_wanted(&mut self) -> Option<(PathBuf, Arc<Manifest>, Rules)> {
        let path = self
            .views
            .keys()
            .find(|path| {
                self.roots
                    .get(*path)
                    .is_some_and(|root| root.git_due && root.latest.is_some())
            })?
            .clone();
        let root = self.roots.get_mut(&path)?;
        root.git_due = false;
        let latest = root.latest.clone()?;
        Some((path, latest, root.rules.clone()))
    }

    /// Takes what git says of `root` (`None`: not a repository, or git could not say).
    pub fn git_read(&mut self, root: &Path, repository: Option<Repository>) {
        if let Some(watched) = self.roots.get_mut(root) {
            let read = repository.map(Arc::new);
            if watched.git != read {
                watched.git = read;
                self.touch();
            }
        }
    }

    /// The root with the latest turn, this run's or one an earlier run listed, which Show
    /// Changes and Review changes open.
    pub fn busiest_root(&self) -> Option<&Path> {
        self.roots
            .values()
            .filter_map(|root| {
                let turned = root.turns.last().map(|turn| turn.started_ms);
                let carried = root.carried.values().map(|listed| listed.changed_ms).max();
                Some((turned.max(carried)?, root))
            })
            .max_by_key(|(started, _)| *started)
            .map(|(_, root)| root.path.as_path())
    }
}

/// `path` relative to `root` with `/` between names; `None` outside it.
pub fn relative_to(root: &Path, path: &Path) -> Option<String> {
    let below = path.strip_prefix(root).ok()?;
    let names: Vec<String> = below
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    (!names.is_empty()).then(|| names.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::scan::Stamp;
    use crate::changes::snapshot::Content;
    use crate::changes::store::Hash;

    pub(crate) fn manifest(taken_ms: u64, files: &[(&str, &str)]) -> Taken {
        let files = files
            .iter()
            .map(|(path, text)| {
                (
                    (*path).to_owned(),
                    Entry {
                        stamp: Stamp {
                            bytes: text.len() as u64,
                            modified_ns: 0,
                        },
                        content: Content::Text(Hash::of(text.as_bytes())),
                    },
                )
            })
            .collect();
        Taken {
            manifest: Manifest { taken_ms, files },
            rules: Rules::default(),
            unreadable: 0,
            partial: false,
        }
    }

    /// Takes every asked snapshot as `files`.
    fn answer(engine: &mut Engine, taken_ms: u64, files: &[(&str, &str)]) -> Vec<Purpose> {
        let mut answered = Vec::new();
        while let Some((request, _)) = engine.next_request() {
            engine.snapshot_taken(&request, manifest(taken_ms, files));
            answered.push(request.purpose);
        }
        answered
    }

    fn key(terminal: &str, number: u32) -> TurnKey {
        TurnKey {
            terminal: terminal.to_owned(),
            number,
        }
    }

    fn engine_with(terminals: &[&str]) -> Engine {
        let mut engine = Engine::default();
        for id in terminals {
            engine.add_terminal(id, Path::new("/p"), id, || None);
        }
        answer(&mut engine, 10, &[("a.txt", "one")]);
        engine
    }

    #[test]
    fn a_hooked_turn_runs_from_prompt_to_stop_with_a_snapshot_at_each_end() {
        let mut engine = engine_with(&["t"]);
        let root = Path::new("/p");
        assert!(engine.roots[root].session_base.is_some());
        engine.signal("t", Signal::Hook(Activity::Working), 100);
        // Nothing changed since the base: it is the turn's start.
        assert!(engine.roots[root].turns[0].before.is_some());
        engine.signal("t", Signal::Hook(Activity::Working), 150);
        engine.signal("t", Signal::Hook(Activity::Finished), 200);
        assert_eq!(
            answer(&mut engine, 210, &[("a.txt", "two")]),
            [Purpose::TurnEnd(key("t", 1))]
        );
        let turn = &engine.roots[root].turns[0];
        assert_eq!((turn.started_ms, turn.ended_ms), (100, Some(200)));
        assert_eq!(turn.after.as_ref().map(|after| after.taken_ms), Some(210));
        // The next prompt is turn 2.
        engine.signal("t", Signal::Hook(Activity::Working), 300);
        assert_eq!(engine.roots[root].turns[1].key, key("t", 2));
    }

    #[test]
    fn a_change_before_a_turn_asks_for_a_fresh_start() {
        let mut engine = engine_with(&["t"]);
        engine.touched(&[PathBuf::from("/p/a.txt")], 50);
        engine.signal("t", Signal::Entered, 60);
        assert!(engine.roots[Path::new("/p")].turns[0].before.is_none());
        assert_eq!(
            answer(&mut engine, 61, &[]),
            [Purpose::TurnStart(key("t", 1))]
        );
    }

    #[test]
    fn a_turn_without_hooks_ends_when_it_settles_or_the_folder_is_quiet() {
        let mut engine = engine_with(&["t"]);
        engine.signal("t", Signal::Entered, 1_000);
        engine.touched(&[PathBuf::from("/p/a.txt")], 2_000);
        engine.tick(4_999);
        assert_eq!(engine.terminals["t"].open, Some(1));
        engine.tick(5_000);
        assert_eq!(engine.terminals["t"].open, None);
        // A program that shows it works keeps its turn through quiet, until it settles.
        engine.signal("t", Signal::Entered, 6_000);
        engine.signal("t", Signal::Unhooked(Activity::Working), 6_100);
        engine.tick(20_000);
        assert_eq!(engine.terminals["t"].open, Some(2));
        engine.signal("t", Signal::Unhooked(Activity::Finished), 21_000);
        assert_eq!(engine.terminals["t"].open, None);
        let ended = engine.roots[Path::new("/p")].turns[1].ended_ms;
        assert_eq!(ended, Some(21_000));
    }

    #[test]
    fn a_hooked_turn_ignores_quiet_and_return() {
        let mut engine = engine_with(&["t"]);
        engine.signal("t", Signal::Entered, 1_000);
        engine.signal("t", Signal::Hook(Activity::Working), 1_100);
        engine.tick(60_000);
        engine.signal("t", Signal::Unhooked(Activity::Idle), 60_100);
        assert_eq!(engine.terminals["t"].open, Some(1));
        engine.signal("t", Signal::Hook(Activity::Finished), 60_200);
        assert_eq!(engine.terminals["t"].open, None);
    }

    #[test]
    fn changes_outside_turns_are_read_once_quiet_and_skipped_paths_are_not_seen() {
        let mut engine = engine_with(&["t"]);
        engine.touched(
            &[
                PathBuf::from("/p/notes.txt"),
                PathBuf::from("/p/node_modules/x.js"),
                PathBuf::from("/elsewhere/y.txt"),
            ],
            1_000,
        );
        assert!(engine.roots[Path::new("/p")].refresh_due);
        engine.tick(1_500);
        assert!(engine.next_request().is_none());
        engine.tick(2_000);
        assert_eq!(answer(&mut engine, 2_001, &[]), [Purpose::Refresh]);
    }

    #[test]
    fn an_edit_the_hook_reports_belongs_to_the_running_turn() {
        let mut engine = engine_with(&["t"]);
        engine.signal("t", Signal::Hook(Activity::Working), 100);
        engine.signal("t", Signal::Edited(PathBuf::from("/p/src/main.rs")), 120);
        engine.signal("t", Signal::Edited(PathBuf::from("/other/x.rs")), 130);
        let turn = &engine.roots[Path::new("/p")].turns[0];
        assert_eq!(turn.reported.iter().collect::<Vec<_>>(), ["src/main.rs"]);
    }

    #[test]
    fn a_stopped_terminal_ends_its_turn_and_its_folder_is_no_longer_watched() {
        let mut engine = engine_with(&["t"]);
        engine.signal("t", Signal::Entered, 100);
        assert_eq!(engine.watched_roots().len(), 1);
        engine.remove_terminal("t", 200);
        assert_eq!(engine.roots[Path::new("/p")].turns[0].ended_ms, Some(200));
        assert!(engine.watched_roots().is_empty());
        assert_eq!(engine.busiest_root(), Some(Path::new("/p")));
    }

    #[test]
    fn terminals_in_one_root_share_it_and_git_s_own_files_are_not_changes() {
        let mut engine = engine_with(&["a", "b"]);
        assert_eq!(engine.roots.len(), 1);
        assert_eq!(engine.roots[Path::new("/p")].running.len(), 2);
        // A commit touches only `.git`: nothing counts as changed.
        engine.touched(&[PathBuf::from("/p/.git/refs/heads/main")], 30);
        assert!(!engine.roots[Path::new("/p")].refresh_due);
    }

    #[test]
    fn a_shown_root_is_watched_and_keeps_its_view_until_its_tab_closes() {
        let mut engine = engine_with(&["t"]);
        let (root, other) = (Path::new("/p"), Path::new("/q"));
        engine.remove_terminal("t", 50);
        assert!(engine.watched_roots().is_empty());
        engine.open_view(root, || None);
        assert_eq!(engine.watched_roots().len(), 1);
        engine.views.get_mut(root).expect("view").within = 3.0;
        // Opening it again from a menu keeps how it was set.
        engine.open_view(root, || None);
        engine.open_view(other, || None);
        assert_eq!(engine.views[root].within, 3.0);
        assert_eq!(engine.watched_roots().len(), 2);
        engine.close_view(root);
        assert_eq!(
            engine.watched_roots().into_iter().collect::<Vec<_>>(),
            [other]
        );
    }
}
