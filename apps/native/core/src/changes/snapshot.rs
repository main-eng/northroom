//! A snapshot of a watched folder: each file's stamp and the hash of its kept contents. A file
//! is read again only when its size or time differ from the last snapshot, as git's index does,
//! or when its time is too close to that snapshot to trust. The latest manifest of each root is
//! kept on disk so a restart reads only what changed.

use crate::changes::scan::{self, Stamp};
use crate::changes::store::{Hash, Store};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

/// Files larger than this are listed by stamp only.
const LARGEST_BYTES: u64 = 8 * 1024 * 1024;
/// A file with a NUL in its first 8 KB is binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// A file changed within this long of the last snapshot may have changed again in the same
/// clock tick, so it is read again.
const RACY_NS: u64 = 2_000_000_000;
/// How long a snapshot's walk may take before it stops, listing what it found.
const WALK_TIME_MS: u64 = 5_000;

/// What is kept of a file's contents.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Content {
    Text(Hash),
    Binary(Hash),
    /// Over `LARGEST_BYTES`.
    TooLarge,
    /// Not read in the snapshot's time budget; the next snapshot reads it.
    Unread,
}

impl Content {
    pub fn hash(&self) -> Option<&Hash> {
        match self {
            Self::Text(hash) | Self::Binary(hash) => Some(hash),
            Self::TooLarge | Self::Unread => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Entry {
    pub stamp: Stamp,
    pub content: Content,
}

impl Entry {
    /// Whether two entries hold different contents. Only a stamp differs between two unread or
    /// too large entries, so that counts.
    pub fn differs(&self, other: &Self) -> bool {
        match (self.content.hash(), other.content.hash()) {
            (Some(mine), Some(theirs)) => mine != theirs,
            _ => self.content != other.content || self.stamp != other.stamp,
        }
    }
}

/// A folder's files at one moment, by path relative to the folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub taken_ms: u64,
    pub files: BTreeMap<String, Entry>,
}

/// How long a snapshot may spend reading files, from a clock in milliseconds.
pub struct Budget<'a> {
    pub clock: &'a dyn crate::changes::Clock,
    pub until_ms: u64,
}

/// What a snapshot found besides the manifest.
pub struct Taken {
    pub manifest: Manifest,
    pub rules: crate::changes::ignore::Rules,
    pub unreadable: usize,
    /// The walk stopped early in a large folder, so some files are not tracked.
    pub partial: bool,
}

/// Takes a snapshot of `root`, reusing what `previous` read of files whose stamps are the
/// same, and keeping new contents in `store`.
pub fn take(root: &Path, previous: &Manifest, store: &Store, budget: &Budget) -> io::Result<Taken> {
    let limit = scan::Limit {
        most_entries: scan::MOST_ENTRIES,
        clock: budget.clock,
        until_ms: budget.clock.now_ms().saturating_add(WALK_TIME_MS),
    };
    let mut walked = scan::walk(root, &limit)?;
    let taken_ms = budget.clock.now_ms();
    let mut files = BTreeMap::new();
    let mut unreadable = walked.unreadable;
    for found in std::mem::take(&mut walked.files) {
        let reused = previous
            .files
            .get(&found.path)
            .filter(|kept| reusable(kept, found.stamp, previous.taken_ms, store));
        let content = match reused {
            Some(kept) => kept.content.clone(),
            None if budget.clock.now_ms() > budget.until_ms => Content::Unread,
            None => match read_content(&root.join(&found.path), found.stamp, store) {
                Ok(content) => content,
                // Gone or locked since the walk: left out, as if never there.
                Err(_) => {
                    unreadable += 1;
                    continue;
                }
            },
        };
        files.insert(
            found.path,
            Entry {
                stamp: found.stamp,
                content,
            },
        );
    }
    keep_unwalked(&mut files, &walked, previous);
    Ok(Taken {
        manifest: Manifest { taken_ms, files },
        rules: walked.rules,
        unreadable,
        partial: walked.partial,
    })
}

/// Keeps what `previous` had of files a stopped walk did not reach (in folders it did not list
/// in full), as they were, so they don't read as deleted.
fn keep_unwalked(files: &mut BTreeMap<String, Entry>, walked: &scan::Walked, previous: &Manifest) {
    if !walked.partial {
        return;
    }
    let unreached = previous
        .files
        .iter()
        .filter(|(path, _)| !walked.listed.contains(scan::folder_of(path)));
    for (path, entry) in unreached {
        files.entry(path.clone()).or_insert_with(|| entry.clone());
    }
}

/// Whether what was kept of a file still stands for it.
fn reusable(kept: &Entry, stamp: Stamp, taken_ms: u64, store: &Store) -> bool {
    let racy = stamp.modified_ns.saturating_add(RACY_NS) >= taken_ms.saturating_mul(1_000_000);
    if kept.stamp != stamp || racy {
        return false;
    }
    match &kept.content {
        Content::Text(hash) | Content::Binary(hash) => store.holds(hash),
        Content::TooLarge => true,
        Content::Unread => false,
    }
}

fn read_content(path: &Path, stamp: Stamp, store: &Store) -> io::Result<Content> {
    if stamp.bytes > LARGEST_BYTES {
        return Ok(Content::TooLarge);
    }
    let bytes = std::fs::read(path)?;
    let hash = store.put(&bytes)?;
    Ok(if is_binary(&bytes) {
        Content::Binary(hash)
    } else {
        Content::Text(hash)
    })
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(BINARY_SNIFF_BYTES).any(|&byte| byte == 0)
}

/// Where `root`'s latest manifest is kept: named by the hash of its path.
fn manifest_path(store: &Store, root: &Path) -> std::path::PathBuf {
    store
        .roots()
        .join(format!("{}.json", Store::root_name(root)))
}

/// The kept latest manifest of `root`, for reading only what changed; `None` when there is none
/// or it cannot be read, as after an update that changed the format. It is only a cache.
pub fn load(store: &Store, root: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(manifest_path(store, root)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(store: &Store, root: &Path, manifest: &Manifest) -> io::Result<()> {
    let path = manifest_path(store, root);
    std::fs::create_dir_all(store.roots())?;
    let staged = path.with_extension("writing");
    std::fs::write(
        &staged,
        serde_json::to_vec(manifest).map_err(io::Error::other)?,
    )?;
    std::fs::rename(staged, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::testing::FixedClock;
    use std::fs;
    use std::time::{Duration, SystemTime};

    /// Sets a file's time to `seconds` after the epoch.
    fn stamp_at(path: &Path, seconds: u64) {
        let file = fs::File::options().write(true).open(path).expect("open");
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
            .expect("time");
    }

    #[test]
    fn only_files_whose_stamps_changed_are_read_again() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let store = Store::at(data.path());
        let root = folder.path();
        fs::write(root.join("a.txt"), "one").expect("write");
        fs::write(root.join("b.bin"), b"x\0y").expect("write");
        stamp_at(&root.join("a.txt"), 1_000);
        stamp_at(&root.join("b.bin"), 1_000);
        let clock = FixedClock::at(2_000_000);
        let budget = Budget {
            clock: &clock,
            until_ms: u64::MAX,
        };
        let first = take(root, &Manifest::default(), &store, &budget).expect("first");
        let a = first.manifest.files["a.txt"].clone();
        assert_eq!(a.content, Content::Text(Hash::of(b"one")));
        assert!(matches!(
            first.manifest.files["b.bin"].content,
            Content::Binary(_)
        ));
        // Same size and time: not read again, even though the bytes differ.
        fs::write(root.join("a.txt"), "two").expect("write");
        stamp_at(&root.join("a.txt"), 1_000);
        let second = take(root, &first.manifest, &store, &budget).expect("second");
        assert_eq!(second.manifest.files["a.txt"], a);
        // A new time: read again.
        stamp_at(&root.join("a.txt"), 1_001);
        let third = take(root, &second.manifest, &store, &budget).expect("third");
        assert_eq!(
            third.manifest.files["a.txt"].content,
            Content::Text(Hash::of(b"two"))
        );
        // A kept object that was pruned is read again.
        fs::remove_file(store.object_path(&Hash::of(b"two"))).expect("prune");
        let fourth = take(root, &third.manifest, &store, &budget).expect("fourth");
        assert!(store.holds(&Hash::of(b"two")));
        assert_eq!(
            fourth.manifest.files["a.txt"],
            third.manifest.files["a.txt"]
        );
    }

    #[test]
    fn a_file_changed_close_to_the_last_snapshot_is_read_again() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let store = Store::at(data.path());
        fs::write(folder.path().join("a.txt"), "one").expect("write");
        stamp_at(&folder.path().join("a.txt"), 1_000);
        // The snapshot is taken in the same second the file changed.
        let clock = FixedClock::at(1_000_500);
        let budget = Budget {
            clock: &clock,
            until_ms: u64::MAX,
        };
        let first = take(folder.path(), &Manifest::default(), &store, &budget).expect("first");
        fs::write(folder.path().join("a.txt"), "two").expect("write");
        stamp_at(&folder.path().join("a.txt"), 1_000);
        let second = take(folder.path(), &first.manifest, &store, &budget).expect("second");
        assert_eq!(
            second.manifest.files["a.txt"].content,
            Content::Text(Hash::of(b"two"))
        );
    }

    #[test]
    fn large_files_are_stamped_only_and_a_spent_budget_leaves_files_unread() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let store = Store::at(data.path());
        let big = fs::File::create(folder.path().join("big.dat")).expect("create");
        big.set_len(LARGEST_BYTES + 1).expect("grow");
        fs::write(folder.path().join("small.txt"), "small").expect("write");
        let clock = FixedClock::at(5_000_000);
        let generous = Budget {
            clock: &clock,
            until_ms: u64::MAX,
        };
        let taken = take(folder.path(), &Manifest::default(), &store, &generous).expect("take");
        assert_eq!(taken.manifest.files["big.dat"].content, Content::TooLarge);
        let spent = Budget {
            clock: &clock,
            until_ms: 0,
        };
        let rushed = take(folder.path(), &Manifest::default(), &store, &spent).expect("take");
        assert_eq!(rushed.manifest.files["small.txt"].content, Content::Unread);
        // The next snapshot reads what was left unread.
        let next = take(folder.path(), &rushed.manifest, &store, &generous).expect("next");
        assert_eq!(
            next.manifest.files["small.txt"].content,
            Content::Text(Hash::of(b"small"))
        );
    }

    #[test]
    fn a_walk_that_stops_early_keeps_what_it_did_not_reach() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let store = Store::at(data.path());
        fs::create_dir_all(folder.path().join("deep")).expect("folder");
        fs::write(folder.path().join("deep/a.txt"), "one").expect("write");
        let clock = FixedClock::at(5_000_000);
        let generous = Budget {
            clock: &clock,
            until_ms: u64::MAX,
        };
        let full = take(folder.path(), &Manifest::default(), &store, &generous).expect("full");
        assert!(!full.partial);
        // Each look at this clock moves it past the walk's time.
        let racing = crate::changes::testing::SteppingClock::new(5_000_000, WALK_TIME_MS + 1);
        let rushed = Budget {
            clock: &racing,
            until_ms: u64::MAX,
        };
        let stopped = take(folder.path(), &full.manifest, &store, &rushed).expect("stopped");
        assert!(stopped.partial);
        assert_eq!(
            stopped.manifest.files["deep/a.txt"],
            full.manifest.files["deep/a.txt"]
        );
    }

    #[test]
    fn a_kept_manifest_comes_back_and_a_broken_one_is_none() {
        let data = tempfile::tempdir().expect("d");
        let store = Store::at(data.path());
        let root = Path::new("/somewhere/project");
        let mut manifest = Manifest {
            taken_ms: 7,
            ..Manifest::default()
        };
        manifest.files.insert(
            "a.txt".into(),
            Entry {
                stamp: Stamp {
                    bytes: 3,
                    modified_ns: 9,
                },
                content: Content::Text(Hash::of(b"abc")),
            },
        );
        save(&store, root, &manifest).expect("save");
        assert_eq!(load(&store, root), Some(manifest));
        fs::write(manifest_path(&store, root), "not json").expect("break");
        assert_eq!(load(&store, root), None);
    }
}
