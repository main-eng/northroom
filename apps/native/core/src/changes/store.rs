//! The snapshot store: file contents kept once each by their SHA-256, in the app's data folder
//! (ADR-0002), never beside the files. `objects/ab/cdef…` holds the bytes, `roots/` the latest
//! manifest of each watched folder, `records/` what each root's Changes list needs across
//! restarts (`record`), and `trashing/` a file's copy for the moment it takes to move it to the
//! system Trash (`actions`). Pruning keeps it to 14 days and 2 GB, and spares what a record names.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const OBJECTS: &str = "objects";
const ROOTS: &str = "roots";
const RECORDS: &str = "records";
const TRASHING: &str = "trashing";
/// What pruning keeps: nothing older than 14 days, and no more than 2 GB.
pub const KEEP_MS: u64 = 14 * 24 * 60 * 60 * 1000;
pub const KEEP_BYTES: u64 = 2 * 1024 * 1024 * 1024;
/// How often a running app prunes again.
pub const PRUNE_EVERY_MS: u64 = 60 * 60 * 1000;

/// The SHA-256 of some bytes, as 64 lowercase hex digits.
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Hash(String);

impl Hash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(crate::digest::sha256_hex(bytes))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The store under one folder.
#[derive(Clone, Debug)]
pub struct Store {
    folder: PathBuf,
}

impl Store {
    pub fn at(folder: &Path) -> Self {
        Self {
            folder: folder.to_path_buf(),
        }
    }

    /// Where the bytes with `hash` are kept.
    pub fn object_path(&self, hash: &Hash) -> PathBuf {
        let (head, rest) = hash.as_str().split_at(2);
        self.folder.join(OBJECTS).join(head).join(rest)
    }

    /// The folder of the watched roots' manifests.
    pub fn roots(&self) -> PathBuf {
        self.folder.join(ROOTS)
    }

    /// Where a copy of a file waits to be moved to the system Trash (`actions`).
    pub fn trashing(&self) -> PathBuf {
        self.folder.join(TRASHING)
    }

    /// The folder of the roots' records (`record`), which pruning leaves alone.
    pub fn records(&self) -> PathBuf {
        self.folder.join(RECORDS)
    }

    /// The name a root's files in `roots/` and `records/` go by: the hash of its path.
    pub fn root_name(root: &Path) -> String {
        Hash::of(root.to_string_lossy().as_bytes()).as_str()[..32].to_owned()
    }

    /// Keeps `bytes` and returns their hash. Bytes already kept are not written again.
    pub fn put(&self, bytes: &[u8]) -> io::Result<Hash> {
        let hash = Hash::of(bytes);
        let path = self.object_path(&hash);
        if path.is_file() {
            return Ok(hash);
        }
        if let Some(folder) = path.parent() {
            fs::create_dir_all(folder)?;
        }
        let staged = path.with_extension("writing");
        fs::write(&staged, bytes)?;
        fs::rename(staged, &path)?;
        Ok(hash)
    }

    pub fn holds(&self, hash: &Hash) -> bool {
        self.object_path(hash).is_file()
    }

    pub fn read(&self, hash: &Hash) -> io::Result<Vec<u8>> {
        fs::read(self.object_path(hash))
    }

    /// Deletes objects and manifests older than `keep_ms` before `now_ms`, then the oldest
    /// until the rest fit in `keep_bytes`, sparing the objects of `referenced`, which a list of
    /// changes still diffs. Returns how many files went.
    pub fn prune(
        &self,
        now_ms: u64,
        keep_ms: u64,
        keep_bytes: u64,
        referenced: &BTreeSet<Hash>,
    ) -> io::Result<usize> {
        let spared: BTreeSet<PathBuf> = referenced
            .iter()
            .map(|hash| self.object_path(hash))
            .collect();
        let mut kept = Vec::new();
        for folder in [self.folder.join(OBJECTS), self.roots()] {
            list_files(&folder, &mut kept)?;
        }
        kept.retain(|file| !spared.contains(&file.path));
        let cutoff = now_ms.saturating_sub(keep_ms);
        let (old, mut young): (Vec<Kept>, Vec<Kept>) =
            kept.into_iter().partition(|file| file.modified_ms < cutoff);
        young.sort_by_key(|file| file.modified_ms);
        let mut total: u64 = young.iter().map(|file| file.bytes).sum();
        let mut gone = 0;
        for file in old {
            remove(&file.path)?;
            gone += 1;
        }
        for file in young {
            if total <= keep_bytes {
                break;
            }
            remove(&file.path)?;
            total -= file.bytes;
            gone += 1;
        }
        Ok(gone)
    }
}

struct Kept {
    path: PathBuf,
    modified_ms: u64,
    bytes: u64,
}

/// Every file under `folder`, at any depth. A missing folder holds none.
fn list_files(folder: &Path, out: &mut Vec<Kept>) -> io::Result<()> {
    let entries = match fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            list_files(&entry.path(), out)?;
            continue;
        }
        out.push(Kept {
            path: entry.path(),
            modified_ms: crate::changes::millis_of(meta.modified()?),
            bytes: meta.len(),
        });
    }
    Ok(())
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn age(path: &Path, modified_ms: u64) {
        let file = fs::File::options().write(true).open(path).expect("open");
        file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(modified_ms))
            .expect("set time");
    }

    #[test]
    fn bytes_are_kept_once_under_their_hash() {
        let folder = tempfile::tempdir().expect("folder");
        let store = Store::at(folder.path());
        let hash = store.put(b"hello").expect("put");
        assert_eq!(
            hash.as_str(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert!(store.object_path(&hash).ends_with(
            "objects/2c/f24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        ));
        age(&store.object_path(&hash), 5);
        // Putting the same bytes again leaves the kept file alone.
        assert_eq!(store.put(b"hello").expect("put again"), hash);
        let modified = fs::metadata(store.object_path(&hash))
            .and_then(|meta| meta.modified())
            .expect("time");
        assert_eq!(crate::changes::millis_of(modified), 5);
        assert_eq!(store.read(&hash).expect("read"), b"hello");
        assert!(!store.holds(&Hash::of(b"other")));
    }

    #[test]
    fn pruning_drops_the_old_then_the_oldest_past_the_size() {
        let folder = tempfile::tempdir().expect("folder");
        let store = Store::at(folder.path());
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let ancient = store.put(b"ancient").expect("put");
        let older = store.put(b"older-10").expect("put");
        let newer = store.put(b"newer-10").expect("put");
        fs::create_dir_all(store.roots()).expect("roots");
        let manifest = store.roots().join("a.json");
        fs::write(&manifest, "{}").expect("manifest");
        age(&store.object_path(&ancient), now - 15 * day);
        age(&manifest, now - 20 * day);
        age(&store.object_path(&older), now - 2 * day);
        age(&store.object_path(&newer), now - day);
        // Room for one of the two young objects: the older one goes.
        let none = BTreeSet::new();
        let gone = store.prune(now, KEEP_MS, 8, &none).expect("prune");
        assert_eq!(gone, 3);
        assert!(!store.holds(&ancient) && !manifest.exists() && !store.holds(&older));
        assert!(store.holds(&newer));
        assert_eq!(
            store.prune(now, KEEP_MS, KEEP_BYTES, &none).expect("again"),
            0
        );
    }

    #[test]
    fn pruning_spares_what_a_list_of_changes_still_diffs() {
        let folder = tempfile::tempdir().expect("folder");
        let store = Store::at(folder.path());
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let baseline = store.put(b"as it was").expect("put");
        let loose = store.put(b"nobody's").expect("put");
        for hash in [&baseline, &loose] {
            age(&store.object_path(hash), now - 30 * day);
        }
        let referenced = BTreeSet::from([baseline.clone()]);
        // Past the age and past the size, it stays while referenced.
        assert_eq!(store.prune(now, KEEP_MS, 0, &referenced).expect("prune"), 1);
        assert!(store.holds(&baseline) && !store.holds(&loose));
    }
}
