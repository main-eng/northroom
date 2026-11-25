//! Runs the changes for the app: the engine behind a lock, one thread that takes snapshots,
//! counts lines, keeps each root's record
//! (`record`) a moment after it changes and prunes the store, and a recursive watcher on each folder a terminal works in or a Changes tab shows.
//! The watcher only marks what changed and when; snapshots read the files. A root's record is
//! read when the root is first watched. Nothing here blocks the UI thread for longer than a
//! lock.

use crate::changes::record::{self, Record};
use crate::changes::snapshot::{self, Budget};
use crate::changes::store::{self, Store};
use crate::changes::turns::{Engine, Signal};
use crate::changes::{view, Clock};
use crate::Relock;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the thread waits for news before ending quiet turns anyway.
const TICK: Duration = Duration::from_millis(500);
/// A burst of file events within this long is handled at once.
const COALESCE: Duration = Duration::from_millis(100);
/// How long one snapshot may spend reading files.
const READ_BUDGET_MS: u64 = 10_000;
/// The records are kept once the engine has been still this long,
const SAVE_AFTER_MS: u64 = 1_000;
/// or at the latest this long after it first changed.
const SAVE_WITHIN_MS: u64 = 5_000;

enum Job {
    /// Something asked for a snapshot or changed the engine.
    Wake,
    /// The watcher saw these paths change.
    Touched(Vec<PathBuf>),
}

struct Shared {
    engine: Mutex<Engine>,
    store: Store,
    clock: Arc<dyn Clock>,
    /// Tells the shell to redraw.
    changed: Box<dyn Fn() + Send + Sync>,
    /// The records as last kept, by root.
    kept: Mutex<BTreeMap<PathBuf, Record>>,
}

#[derive(Default)]
struct Watching {
    watcher: Option<RecommendedWatcher>,
    watched: BTreeSet<PathBuf>,
}

pub struct Service {
    shared: Arc<Shared>,
    jobs: Sender<Job>,
    watching: Mutex<Watching>,
}

impl Service {
    /// Starts the changes with their store in `folder`. `changed` is called on the changes'
    /// thread whenever what the view shows may have changed.
    pub fn start(
        folder: &Path,
        clock: Arc<dyn Clock>,
        changed: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (jobs, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            engine: Mutex::new(Engine::default()),
            store: Store::at(folder),
            clock,
            changed: Box::new(changed),
            kept: Mutex::new(BTreeMap::new()),
        });
        let worker = Arc::clone(&shared);
        let started = std::thread::Builder::new()
            .name("northroom-changes".into())
            .spawn(move || run(&worker, &receiver));
        if let Err(error) = started {
            eprintln!("the changes could not start their thread: {error}");
        }
        let watcher = start_watcher(jobs.clone());
        Self {
            shared,
            jobs,
            watching: Mutex::new(Watching {
                watcher,
                watched: BTreeSet::new(),
            }),
        }
    }

    pub fn store(&self) -> &Store {
        &self.shared.store
    }

    pub fn now_ms(&self) -> u64 {
        self.shared.clock.now_ms()
    }

    pub fn clock(&self) -> &dyn Clock {
        self.shared.clock.as_ref()
    }

    /// Reads the engine.
    pub fn read<T>(&self, action: impl FnOnce(&Engine) -> T) -> T {
        action(&self.shared.engine.relock())
    }

    /// Changes the engine, then watches the folders it wants and wakes the thread for any
    /// snapshot it asked for.
    pub fn change<T>(&self, action: impl FnOnce(&mut Engine) -> T) -> T {
        let (result, wanted) = {
            let mut engine = self.shared.engine.relock();
            let result = action(&mut engine);
            (result, engine.watched_roots())
        };
        self.watch(wanted);
        // The thread only ends with the service.
        let _ = self.jobs.send(Job::Wake);
        result
    }

    /// Follows terminal `id`, whose shell runs in `root`, credited as `credit` (its terminal
    /// file's path relative to `root`).
    pub fn add_terminal(&self, id: &str, root: &Path, credit: &str) {
        self.change(|engine| {
            self.take_up(engine, root);
            engine.add_terminal(id, root, credit, || None);
        });
    }

    /// Watches `root` unless it is watched, reading the snapshot and the record an earlier run
    /// kept of it.
    fn take_up(&self, engine: &mut Engine, root: &Path) {
        if engine.roots.contains_key(root) {
            return;
        }
        let store = &self.shared.store;
        engine.watch_root(root, || snapshot::load(store, root));
        let kept = record::load(store, root, self.now_ms());
        self.shared
            .kept
            .relock()
            .insert(root.to_path_buf(), kept.clone());
        engine.restore(root, kept);
    }

    /// How many files `root` lists (`view::waiting`), taking it up first.
    pub fn waiting(&self, root: &Path) -> usize {
        if !self.read(|engine| engine.roots.contains_key(root)) {
            self.change(|engine| self.take_up(engine, root));
        }
        self.read(|engine| view::waiting(engine, root))
    }

    /// Keeps every root's record now, as when the app quits.
    pub fn save_now(&self) {
        save_records(&self.shared);
    }

    pub fn remove_terminal(&self, id: &str) {
        let now_ms = self.now_ms();
        self.change(|engine| engine.remove_terminal(id, now_ms));
    }

    pub fn signal(&self, id: &str, signal: Signal) {
        let now_ms = self.now_ms();
        self.change(|engine| engine.signal(id, signal, now_ms));
    }

    /// Sets how `root`'s tab is scrolled, if a tab shows it. Nothing is watched, taken or
    /// redrawn for it, so scrolling can report every frame.
    pub fn set_view(&self, root: &Path, set: impl FnOnce(&mut crate::changes::view::ViewState)) {
        if let Some(view) = self.shared.engine.relock().views.get_mut(root) {
            set(view);
        }
    }

    /// Shows `root` in a Changes tab, reading it now.
    pub fn open_view(&self, root: &Path) {
        self.change(|engine| {
            self.take_up(engine, root);
            engine.open_view(root, || None);
        });
    }

    /// Watches exactly `wanted`, recursively.
    fn watch(&self, wanted: BTreeSet<PathBuf>) {
        let mut watching = self.watching.relock();
        let Watching { watcher, watched } = &mut *watching;
        let Some(watcher) = watcher.as_mut() else {
            return;
        };
        for gone in watched.difference(&wanted) {
            // A folder deleted while watched is already unwatched.
            let _ = watcher.unwatch(gone);
        }
        for added in wanted.difference(watched) {
            if let Err(error) = watcher.watch(added, RecursiveMode::Recursive) {
                eprintln!("the changes could not watch {}: {error}", added.display());
            }
        }
        *watched = wanted;
    }
}

fn start_watcher(jobs: Sender<Job>) -> Option<RecommendedWatcher> {
    let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let Ok(event) = event else {
            return;
        };
        if !matches!(event.kind, notify::EventKind::Access(_)) {
            let _ = jobs.send(Job::Touched(event.paths));
        }
    });
    match watcher {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            eprintln!("the changes run without a folder watcher: {error}");
            None
        }
    }
}

/// The changes' thread: takes file events in bursts, ends quiet turns, takes the snapshots
/// asked for, counts their lines and prunes the store, until the service is gone.
fn run(shared: &Shared, receiver: &Receiver<Job>) {
    let mut pruned_ms = None;
    let mut saving = Saving::default();
    loop {
        match receiver.recv_timeout(TICK) {
            Ok(Job::Touched(paths)) => {
                let paths = gather(paths, receiver);
                let now_ms = shared.clock.now_ms();
                shared.engine.relock().touched(&paths, now_ms);
            }
            Ok(Job::Wake) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        let now_ms = shared.clock.now_ms();
        shared.engine.relock().tick(now_ms);
        if take_snapshots(shared) {
            count_lines(shared);
            (shared.changed)();
        }
        save_when_due(shared, &mut saving);
        prune_when_due(shared, &mut pruned_ms);
    }
}

/// When the engine last changed, to keep the records a moment after.
#[derive(Default)]
struct Saving {
    revision: u64,
    /// When the revision first and last moved since the records were kept.
    first_ms: Option<u64>,
    last_ms: u64,
}

fn save_when_due(shared: &Shared, saving: &mut Saving) {
    let now_ms = shared.clock.now_ms();
    let revision = shared.engine.relock().revision();
    if revision != saving.revision {
        saving.revision = revision;
        saving.first_ms.get_or_insert(now_ms);
        saving.last_ms = now_ms;
    }
    let Some(first_ms) = saving.first_ms else {
        return;
    };
    let still = now_ms.saturating_sub(saving.last_ms) >= SAVE_AFTER_MS;
    if still || now_ms.saturating_sub(first_ms) >= SAVE_WITHIN_MS {
        saving.first_ms = None;
        save_records(shared);
    }
}

/// Writes the record of every root whose record changed since it was last kept.
fn save_records(shared: &Shared) {
    let now_ms = shared.clock.now_ms();
    let records: Vec<(PathBuf, Record)> = {
        let engine = shared.engine.relock();
        engine
            .roots
            .values()
            .map(|root| (root.path.clone(), view::record(root, now_ms)))
            .collect()
    };
    let mut kept = shared.kept.relock();
    for (root, record) in records {
        if kept.get(&root) == Some(&record) {
            continue;
        }
        match record::save(&shared.store, &root, &record) {
            Ok(()) => {
                kept.insert(root, record);
            }
            Err(error) => eprintln!(
                "the changes could not keep the record of {}: {error}",
                root.display()
            ),
        }
    }
}

/// `first` and every path that arrives within the burst after it.
fn gather(mut paths: Vec<PathBuf>, receiver: &Receiver<Job>) -> Vec<PathBuf> {
    let until = std::time::Instant::now() + COALESCE;
    while let Some(left) = until.checked_duration_since(std::time::Instant::now()) {
        match receiver.recv_timeout(left) {
            Ok(Job::Touched(more)) => paths.extend(more),
            Ok(Job::Wake) => {}
            Err(_) => break,
        }
    }
    paths
}

/// Takes every snapshot the engine asked for. Returns whether any was taken.
fn take_snapshots(shared: &Shared) -> bool {
    let mut took = false;
    loop {
        let next = shared.engine.relock().next_request();
        let Some((request, previous)) = next else {
            return took;
        };
        let budget = Budget {
            clock: shared.clock.as_ref(),
            until_ms: shared.clock.now_ms() + READ_BUDGET_MS,
        };
        let taken = match snapshot::take(&request.root, &previous, &shared.store, &budget) {
            Ok(taken) => taken,
            Err(error) => {
                eprintln!(
                    "the changes could not read {}: {error}",
                    request.root.display()
                );
                continue;
            }
        };
        if taken.partial {
            eprintln!(
                "the changes track only part of {}: it holds too many files to walk in time",
                request.root.display()
            );
        }
        if taken.unreadable > 0 {
            eprintln!(
                "the changes left out {} unreadable paths in {}",
                taken.unreadable,
                request.root.display()
            );
        }
        if let Err(error) = snapshot::save(&shared.store, &request.root, &taken.manifest) {
            eprintln!(
                "the changes could not keep the snapshot of {}: {error}",
                request.root.display()
            );
        }
        shared.engine.relock().snapshot_taken(&request, taken);
        took = true;
    }
}

/// Counts the lines of every changed text not yet counted.
fn count_lines(shared: &Shared) {
    let wanted = view::uncounted(&shared.engine.relock());
    for pair in wanted {
        match view::count_lines(&shared.store, &pair, shared.clock.as_ref()) {
            Ok(counts) => shared.engine.relock().counts.insert(pair, counts),
            Err(error) => eprintln!("the changes could not count a file's lines: {error}"),
        }
    }
    shared.engine.relock().touch();
}

fn prune_when_due(shared: &Shared, pruned_ms: &mut Option<u64>) {
    let now_ms = shared.clock.now_ms();
    if pruned_ms.is_some_and(|pruned| now_ms.saturating_sub(pruned) < store::PRUNE_EVERY_MS) {
        return;
    }
    *pruned_ms = Some(now_ms);
    let mut referenced = record::referenced(&shared.store);
    referenced.extend(view::referenced(&shared.engine.relock()));
    if let Err(error) = shared
        .store
        .prune(now_ms, store::KEEP_MS, store::KEEP_BYTES, &referenced)
    {
        eprintln!("the changes could not prune their store: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::view::list;
    use crate::changes::SystemClock;
    use crate::terminal::activity::Activity;
    use std::time::Instant;

    /// Waits up to 20 s for `done`.
    fn wait_for(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn a_turn_in_a_watched_folder_lists_what_it_changed_with_its_lines_counted() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let root = folder.path().canonicalize().expect("real path");
        std::fs::write(root.join("a.txt"), "one\n").expect("write");
        let service = Service::start(data.path(), Arc::new(SystemClock), || {});
        service.add_terminal("t", &root, "Terminal.nterm");
        assert!(wait_for(
            || service.read(|engine| engine.roots[&root].session_base.is_some())
        ));
        service.open_view(&root);
        service.signal("t", Signal::Hook(Activity::Working));
        assert!(wait_for(
            || service.read(|engine| engine.roots[&root].turns[0].before.is_some())
        ));
        std::fs::write(root.join("a.txt"), "one\ntwo\n").expect("write");
        service.signal("t", Signal::Edited(root.join("a.txt")));
        service.signal("t", Signal::Hook(Activity::Finished));
        let counted = || {
            service.read(|engine| {
                list(engine, &root)
                    .first()
                    .is_some_and(|row| row.counts == Some((1, 0)))
            })
        };
        assert!(
            wait_for(counted),
            "the turn's change was not listed and counted"
        );
        assert!(snapshot::load(service.store(), &root).is_some());
    }

    #[test]
    fn what_a_run_listed_and_kept_is_there_after_a_restart() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let root = folder.path().canonicalize().expect("real path");
        for name in ["a.txt", "b.txt"] {
            std::fs::write(root.join(name), "one\n").expect("write");
        }
        let rows = |service: &Service| {
            service.read(|engine| {
                list(engine, &root)
                    .into_iter()
                    .map(|row| row.change.path)
                    .collect::<Vec<_>>()
            })
        };
        {
            let service = Service::start(data.path(), Arc::new(SystemClock), || {});
            service.add_terminal("t1", &root, "Terminal.nterm");
            service.open_view(&root);
            assert!(wait_for(
                || service.read(|engine| engine.roots[&root].session_base.is_some())
            ));
            service.signal("t1", Signal::Hook(Activity::Working));
            assert!(wait_for(
                || service.read(|engine| engine.roots[&root].turns[0].before.is_some())
            ));
            for name in ["a.txt", "b.txt"] {
                std::fs::write(root.join(name), "one\ntwo\n").expect("write");
            }
            service.signal("t1", Signal::Hook(Activity::Finished));
            assert!(wait_for(|| rows(&service).len() == 2));
            let now_ms = service.now_ms();
            service.change(|engine| {
                let b = list(engine, &root)
                    .into_iter()
                    .find(|row| row.change.path == "b.txt")
                    .expect("b listed");
                engine.keep(&root, [(b.change.path, b.change.after)], now_ms);
            });
            assert_eq!(service.waiting(&root), 1);
            service.save_now();
        }
        let kept = record::load(&Store::at(data.path()), &root, SystemClock.now_ms());
        assert_eq!(kept.listed.keys().collect::<Vec<_>>(), ["a.txt"]);
        assert_eq!(
            kept.listed["a.txt"].terminals,
            BTreeSet::from(["Terminal.nterm".to_owned()])
        );
        // The next run counts the file before it has read the folder, then lists it.
        let service = Service::start(data.path(), Arc::new(SystemClock), || {});
        assert_eq!(service.waiting(&root), 1);
        service.open_view(&root);
        assert!(wait_for(|| rows(&service) == ["a.txt"]));
        assert!(service.read(|engine| engine.roots[&root].kept.contains_key("b.txt")));
        // Keeping it empties the list, and the next run still lists nothing.
        let now_ms = service.now_ms();
        assert!(service.change(|engine| view::keep_file(engine, &root, "a.txt", now_ms)));
        assert_eq!(service.waiting(&root), 0);
        service.save_now();
        drop(service);
        let service = Service::start(data.path(), Arc::new(SystemClock), || {});
        assert_eq!(service.waiting(&root), 0);
        service.open_view(&root);
        assert!(wait_for(|| service.read(|engine| {
            let watched = &engine.roots[&root];
            watched.session_base.is_some() && watched.latest.is_some()
        })));
        assert!(rows(&service).is_empty());
    }
}
