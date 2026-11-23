//! What a root's Changes list needs across restarts, kept in the app's data folder beside the
//! snapshot store (`records/<root's name>.json`, ADR-0002), never beside the files: each listed
//! file's baseline (the entry it is diffed from), the terminals credited with it, when a turn
//! first and last changed it, and what the person kept of each file. Turns themselves live only in
//! memory; this is what is left of them once the app quits.
//!
//! A terminal is named by its terminal file's path relative to the root, which outlives the
//! ids a run gives terminals. Entries older than the store's 14 days are dropped, so nothing
//! points at pruned contents, and the store spares every object a record names
//! (`Store::prune`). The record is written whole, to a staged file renamed over the old one.

use crate::changes::snapshot::Entry;
use crate::changes::store::{self, Hash, Store};
use crate::changes::turns::Kept;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

/// The format's version; a record of another is read as empty, except the earlier ones, whose
/// marks (version 1's `reviewed`, version 2's `cleared`) meant what keeping a whole file means
/// now and are read as kept marks (`migrate`).
const VERSION: u32 = 3;
const READ_VERSIONS: [u32; 3] = [1, 2, VERSION];

/// A file listed when the record was written.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Listed {
    /// What it is diffed from: its kept entry, else as it was before the first turn that
    /// changed it; `None` when it did not exist.
    pub baseline: Option<Entry>,
    /// The terminals credited with it, by terminal file path relative to the root.
    pub terminals: BTreeSet<String>,
    /// When the first turn that changed it since it was kept started; orders the list.
    pub entered_ms: u64,
    /// When a turn last changed it.
    pub changed_ms: u64,
    /// Two terminals changed it at the same time.
    pub contested: bool,
}

/// One root's record.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Record {
    pub version: u32,
    /// The files listed, by path relative to the root.
    pub listed: BTreeMap<String, Listed>,
    /// What the person kept of each file, by path (`reviewed` in version 1, `cleared` in 2).
    #[serde(alias = "reviewed", alias = "cleared")]
    pub kept: BTreeMap<String, Kept>,
}

impl Record {
    pub fn new(listed: BTreeMap<String, Listed>, kept: BTreeMap<String, Kept>) -> Self {
        Self {
            version: VERSION,
            listed,
            kept,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.listed.is_empty() && self.kept.is_empty()
    }

    /// A record read in an earlier version as this one: each of its marks kept the whole file,
    /// so turns count from when it was made.
    fn migrate(mut self) -> Self {
        if self.version < VERSION {
            for kept in self.kept.values_mut() {
                kept.since_ms = kept.at_ms;
            }
        }
        self.version = VERSION;
        self
    }

    /// Drops what is older than the store keeps (`store::KEEP_MS`) at `now_ms`.
    pub fn expire(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(store::KEEP_MS);
        self.listed.retain(|_, listed| listed.changed_ms >= cutoff);
        self.kept.retain(|_, kept| kept.at_ms >= cutoff);
    }

    /// Every kept object the record names.
    pub fn hashes(&self) -> impl Iterator<Item = &Hash> {
        let baselines = self
            .listed
            .values()
            .filter_map(|listed| listed.baseline.as_ref());
        let kept = self.kept.values().filter_map(|mark| mark.entry.as_ref());
        baselines
            .chain(kept)
            .filter_map(|entry| entry.content.hash())
    }
}

/// Where `root`'s record is kept.
pub fn path(store: &Store, root: &Path) -> PathBuf {
    store
        .records()
        .join(format!("{}.json", Store::root_name(root)))
}

/// `root`'s record as of `now_ms`, without what has expired; empty when there is none or it
/// cannot be read.
pub fn load(store: &Store, root: &Path, now_ms: u64) -> Record {
    let read = std::fs::read_to_string(path(store, root))
        .ok()
        .and_then(|text| serde_json::from_str::<Record>(&text).ok())
        .filter(|record| READ_VERSIONS.contains(&record.version))
        .map(Record::migrate);
    let mut record = read.unwrap_or_default();
    record.expire(now_ms);
    record
}

/// Keeps `record` as `root`'s: written to a staged file, then renamed over the old one, so a
/// reader never sees half of it. An empty record removes the file.
pub fn save(store: &Store, root: &Path, record: &Record) -> io::Result<()> {
    let path = path(store, root);
    if record.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    std::fs::create_dir_all(store.records())?;
    let staged = path.with_extension("writing");
    let text = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    std::fs::write(&staged, text)?;
    std::fs::rename(staged, path)
}

/// Every object any kept record names, for pruning to spare.
pub fn referenced(store: &Store) -> BTreeSet<Hash> {
    let Ok(entries) = std::fs::read_dir(store.records()) else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ending| ending == "json")
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Record>(&text).ok())
        .flat_map(|record| record.hashes().cloned().collect::<Vec<_>>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::scan::Stamp;
    use crate::changes::snapshot::Content;

    const DAY: u64 = 24 * 60 * 60 * 1000;

    fn entry(text: &str) -> Entry {
        Entry {
            stamp: Stamp {
                bytes: text.len() as u64,
                modified_ns: 7,
            },
            content: Content::Text(Hash::of(text.as_bytes())),
        }
    }

    fn listed(baseline: Option<Entry>, changed_ms: u64) -> Listed {
        Listed {
            baseline,
            terminals: BTreeSet::from(["Research/Terminal.nterm".to_owned()]),
            entered_ms: changed_ms - 5,
            changed_ms,
            contested: false,
        }
    }

    fn sample(now: u64) -> Record {
        Record::new(
            BTreeMap::from([
                ("notes.md".to_owned(), listed(Some(entry("one")), now - DAY)),
                ("new.md".to_owned(), listed(None, now - 2 * DAY)),
            ]),
            BTreeMap::from([(
                "done.md".to_owned(),
                Kept {
                    entry: Some(entry("done")),
                    at_ms: now - DAY,
                    since_ms: now - DAY,
                },
            )]),
        )
    }

    #[test]
    fn a_saved_record_loads_as_it_was_and_an_empty_one_removes_the_file() {
        let data = tempfile::tempdir().expect("data");
        let store = Store::at(data.path());
        let root = Path::new("/drive");
        let now = 100 * DAY;
        assert_eq!(load(&store, root, now), Record::default());
        let record = sample(now);
        save(&store, root, &record).expect("save");
        assert_eq!(load(&store, root, now), record);
        assert!(path(&store, root).starts_with(data.path().join("records")));
        save(&store, root, &Record::new(BTreeMap::new(), BTreeMap::new())).expect("empty");
        assert!(!path(&store, root).exists());
    }

    #[test]
    fn a_record_is_replaced_whole_and_a_broken_or_staged_one_is_never_read() {
        let data = tempfile::tempdir().expect("data");
        let store = Store::at(data.path());
        let root = Path::new("/drive");
        let now = 100 * DAY;
        save(&store, root, &sample(now)).expect("save");
        // A write cut short leaves only its staged file; the kept record still reads.
        let staged = path(&store, root).with_extension("writing");
        std::fs::write(&staged, "{\"version\":1,\"lis").expect("half");
        assert_eq!(load(&store, root, now), sample(now));
        // The next save replaces both.
        let mut later = sample(now);
        later.listed.remove("new.md");
        save(&store, root, &later).expect("again");
        assert!(!staged.exists());
        assert_eq!(load(&store, root, now), later);
        // A record that does not parse, or of another version, reads as empty.
        std::fs::write(path(&store, root), "{not json").expect("broken");
        assert_eq!(load(&store, root, now), Record::default());
        let other = serde_json::to_string(&Record {
            version: 99,
            ..sample(now)
        })
        .expect("json");
        std::fs::write(path(&store, root), other).expect("other");
        assert_eq!(load(&store, root, now), Record::default());
    }

    /// `sample(now)` as version `version` wrote it: its marks under `name`, without `since_ms`.
    fn written_by(version: u32, name: &str, now: u64) -> String {
        let mut old = serde_json::to_value(sample(now)).expect("json");
        old["version"] = version.into();
        let mut marks = old["kept"].take();
        for mark in marks.as_object_mut().expect("marks").values_mut() {
            mark.as_object_mut().expect("mark").remove("since_ms");
        }
        old.as_object_mut().expect("object").remove("kept");
        old[name] = marks;
        old.to_string()
    }

    #[test]
    fn earlier_records_marks_read_as_whole_files_kept() {
        let data = tempfile::tempdir().expect("data");
        let store = Store::at(data.path());
        let root = Path::new("/drive");
        let now = 100 * DAY;
        std::fs::create_dir_all(store.records()).expect("records");
        // Version 2's cleared marks and version 1's reviewed ones.
        for (version, name) in [(2, "cleared"), (1, "reviewed")] {
            std::fs::write(path(&store, root), written_by(version, name, now)).expect("old");
            let read = load(&store, root, now);
            assert_eq!(read, sample(now), "version {version}");
            assert_eq!(read.version, 3);
            assert_eq!(read.kept["done.md"].since_ms, now - DAY);
        }
        assert_eq!(referenced(&store).len(), 2);
        // Saved again, it is version 3 with its marks as `kept`.
        save(&store, root, &load(&store, root, now)).expect("save");
        let text = std::fs::read_to_string(path(&store, root)).expect("read");
        assert!(text.contains("\"version\": 3") && text.contains("\"kept\""));
        assert!(!text.contains("cleared"));
    }

    #[test]
    fn entries_past_the_store_s_14_days_are_dropped() {
        let data = tempfile::tempdir().expect("data");
        let store = Store::at(data.path());
        let root = Path::new("/drive");
        let now = 100 * DAY;
        save(&store, root, &sample(now)).expect("save");
        let later = load(&store, root, now + 12 * DAY + DAY / 2);
        assert_eq!(later.listed.keys().collect::<Vec<_>>(), ["notes.md"]);
        assert!(later.kept.contains_key("done.md"));
        assert!(load(&store, root, now + 14 * DAY).is_empty());
    }

    #[test]
    fn pruning_spares_every_object_a_kept_record_names() {
        let data = tempfile::tempdir().expect("data");
        let store = Store::at(data.path());
        let now = 100 * DAY;
        let one = store.put(b"one").expect("put");
        let done = store.put(b"done").expect("put");
        let loose = store.put(b"loose").expect("put");
        save(&store, Path::new("/drive"), &sample(now)).expect("save");
        let referenced = referenced(&store);
        assert_eq!(referenced, BTreeSet::from([one.clone(), done.clone()]));
        // The size allows none.
        store
            .prune(now + 30 * DAY, store::KEEP_MS, 0, &referenced)
            .expect("prune");
        assert!(store.holds(&one) && store.holds(&done) && !store.holds(&loose));
        assert!(path(&store, Path::new("/drive")).exists());
    }
}
