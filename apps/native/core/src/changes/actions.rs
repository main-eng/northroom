//! What the Changes tab does to the files on disk: Undo puts a file, or one hunk of it, back as
//! it is diffed from (the version an agent started from, or what the person last kept), and ⌘Z
//! right after puts the Undo back (`restore`). Undo asks nothing first: before it writes over a
//! file, the file's version then goes to the system Trash, as the drive's own deletes do, and
//! its bytes to the store, so nothing is lost for good (ADR-0002).

use crate::changes::diff::{self, Hunk};
use crate::changes::store::{Hash, Store};
use crate::changes::view::FileChange;
use crate::strings::{self, key};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// Why an action could not be done.
#[derive(Debug)]
pub enum ActionError {
    /// The file's earlier contents were not kept: too large, unread or pruned.
    NotKept,
    /// The file changed where the hunk is, so its lines are no longer where they were.
    ContextChanged,
    /// The file changed after the Undo, so ⌘Z leaves it.
    ChangedSince,
    /// No system trash to keep the file's version in.
    NoTrash,
    Failed(io::Error),
}

impl ActionError {
    /// What went wrong, in `language`.
    pub fn say(&self, language: &str) -> String {
        let words = match self {
            Self::NotKept => key::CHANGES_UNDO_NOT_KEPT,
            Self::ContextChanged => key::CHANGES_UNDO_CONTEXT_CHANGED,
            Self::ChangedSince => key::CHANGES_UNDO_CHANGED_SINCE,
            Self::NoTrash => key::CHANGES_UNDO_NO_TRASH,
            Self::Failed(error) => return error.to_string(),
        };
        strings::text(language, words).to_owned()
    }
}

impl fmt::Display for ActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.say(crate::strings::LANGUAGES[0]))
    }
}

impl From<io::Error> for ActionError {
    fn from(error: io::Error) -> Self {
        Self::Failed(error)
    }
}

/// Moves a file to the trash, as the drive's own deletes do.
pub type Trash<'a> = &'a dyn Fn(&Path) -> io::Result<()>;

/// What an Undo did to a file, for ⌘Z: its contents before and after, by hash in the store
/// (`None` where there was no file).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Undone {
    pub path: PathBuf,
    pub before: Option<Hash>,
    pub after: Option<Hash>,
}

/// Puts `change`'s file in `root` back as it is diffed from: its earlier contents written over
/// it, a deleted file restored, an added file moved to the trash. Its version now goes to the
/// trash first.
pub fn undo_file(
    root: &Path,
    change: &FileChange,
    store: &Store,
    trash: Option<Trash>,
) -> Result<Undone, ActionError> {
    let path = root.join(&change.path);
    let current = read_if_there(&path)?;
    let before = current
        .as_deref()
        .map(|bytes| store.put(bytes))
        .transpose()?;
    let Some(earlier) = &change.before else {
        let trash = trash.ok_or(ActionError::NoTrash)?;
        if current.is_some() {
            trash(&path)?;
        }
        return Ok(Undone {
            path,
            before,
            after: None,
        });
    };
    let hash = earlier.content.hash().ok_or(ActionError::NotKept)?;
    let bytes = store.read(hash).map_err(|_| ActionError::NotKept)?;
    write_over(&path, current.as_deref(), &bytes, store, trash)?;
    Ok(Undone {
        path,
        before,
        after: Some(hash.clone()),
    })
}

/// Undoes `hunk` of `change` in its file in `root` as it is now; refuses when the file changed
/// where the hunk is. Undoing an added file's only hunk moves the file to the trash.
pub fn undo_hunk(
    root: &Path,
    change: &FileChange,
    hunk: &Hunk,
    store: &Store,
    trash: Option<Trash>,
) -> Result<Undone, ActionError> {
    let path = root.join(&change.path);
    let current = read_if_there(&path)?.ok_or(ActionError::ContextChanged)?;
    let text = std::str::from_utf8(&current).map_err(|_| ActionError::ContextChanged)?;
    let undone = diff::undo_hunk(text, hunk).ok_or(ActionError::ContextChanged)?;
    if undone.is_empty() && change.before.is_none() {
        return undo_file(root, change, store, trash);
    }
    let before = store.put(&current)?;
    write_over(&path, Some(&current), undone.as_bytes(), store, trash)?;
    Ok(Undone {
        path,
        before: Some(before),
        after: Some(store.put(undone.as_bytes())?),
    })
}

/// Puts back what `undone` replaced, as ⌘Z right after an Undo does; refuses when the file
/// changed since. A file the Undo restored goes to the trash again.
pub fn restore(undone: &Undone, store: &Store, trash: Option<Trash>) -> Result<(), ActionError> {
    let current = read_if_there(&undone.path)?;
    if current.as_deref().map(Hash::of) != undone.after {
        return Err(ActionError::ChangedSince);
    }
    match &undone.before {
        Some(hash) => {
            let bytes = store.read(hash).map_err(|_| ActionError::NotKept)?;
            write_whole(&undone.path, &bytes)
        }
        None => {
            let trash = trash.ok_or(ActionError::NoTrash)?;
            Ok(trash(&undone.path)?)
        }
    }
}

/// The file's bytes; `None` when there is no file.
fn read_if_there(path: &Path) -> Result<Option<Vec<u8>>, ActionError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Writes `bytes` over the file at `path`, whose bytes now are `current`, after sending a copy
/// of them to the trash.
fn write_over(
    path: &Path,
    current: Option<&[u8]>,
    bytes: &[u8],
    store: &Store,
    trash: Option<Trash>,
) -> Result<(), ActionError> {
    if let Some(current) = current {
        trash_copy(path, current, store, trash)?;
    }
    write_whole(path, bytes)
}

/// Moves a copy of `bytes` named as the file at `path` to the trash, staged in the store's
/// folder so the drive never sees it.
fn trash_copy(
    path: &Path,
    bytes: &[u8],
    store: &Store,
    trash: Option<Trash>,
) -> Result<(), ActionError> {
    let trash = trash.ok_or(ActionError::NoTrash)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("the path has no name"))?;
    let folder = store.trashing().join(Hash::of(bytes).as_str());
    std::fs::create_dir_all(&folder)?;
    let copy = folder.join(name);
    std::fs::write(&copy, bytes)?;
    let trashed = trash(&copy);
    if let Err(error) = std::fs::remove_dir_all(&folder) {
        eprintln!(
            "the changes could not tidy {} after trashing: {error}",
            folder.display()
        );
    }
    Ok(trashed?)
}

/// Writes `bytes` as the whole file, through a staged copy beside it so a failure leaves the
/// file as it was.
fn write_whole(path: &Path, bytes: &[u8]) -> Result<(), ActionError> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let mut staged = path.as_os_str().to_owned();
    staged.push(".northroom-undo");
    std::fs::write(&staged, bytes)?;
    std::fs::rename(&staged, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::scan::Stamp;
    use crate::changes::snapshot::{Content, Entry};
    use crate::changes::testing::FixedClock;
    use std::cell::RefCell;
    use std::fs;

    fn kept(store: &Store, text: &str) -> Option<Entry> {
        let hash = store.put(text.as_bytes()).expect("put");
        Some(Entry {
            stamp: Stamp {
                bytes: text.len() as u64,
                modified_ns: 0,
            },
            content: Content::Text(hash),
        })
    }

    /// A trash that moves files into `folder`, as `NR_TRASH` does, and the names it took.
    struct Bin {
        folder: tempfile::TempDir,
        took: RefCell<Vec<String>>,
    }

    impl Bin {
        fn new() -> Self {
            Self {
                folder: tempfile::tempdir().expect("bin"),
                took: RefCell::new(Vec::new()),
            }
        }

        fn trash(&self, path: &Path) -> io::Result<()> {
            let name = path.file_name().expect("name").to_string_lossy();
            let target = self
                .folder
                .path()
                .join(format!("{}-{name}", self.took.borrow().len()));
            fs::rename(path, &target)?;
            self.took
                .borrow_mut()
                .push(fs::read_to_string(&target).unwrap_or_default());
            Ok(())
        }
    }

    struct Setup {
        folder: tempfile::TempDir,
        _data: tempfile::TempDir,
        store: Store,
        bin: Bin,
    }

    fn setup() -> Setup {
        let data = tempfile::tempdir().expect("d");
        Setup {
            folder: tempfile::tempdir().expect("f"),
            store: Store::at(data.path()),
            _data: data,
            bin: Bin::new(),
        }
    }

    impl Setup {
        fn file(&self) -> PathBuf {
            self.folder.path().join("a.txt")
        }

        fn read(&self) -> String {
            fs::read_to_string(self.file()).expect("read")
        }
    }

    #[test]
    fn undoing_a_file_writes_back_its_earlier_contents_after_trashing_a_copy() {
        let at = setup();
        let trash = |path: &Path| at.bin.trash(path);
        fs::write(at.file(), "after").expect("write");
        let change = FileChange {
            path: "a.txt".into(),
            before: kept(&at.store, "before"),
            after: kept(&at.store, "after"),
        };
        fs::write(at.file(), "after, then more").expect("write");
        let undone = undo_file(at.folder.path(), &change, &at.store, Some(&trash)).expect("undo");
        assert_eq!(at.read(), "before");
        // The version it replaced is in the trash and the store; nothing is left beside it.
        assert_eq!(*at.bin.took.borrow(), ["after, then more"]);
        assert!(at.store.holds(&Hash::of(b"after, then more")));
        assert_eq!(fs::read_dir(at.folder.path()).expect("list").count(), 1);
        assert!(!at
            .store
            .trashing()
            .join(Hash::of(b"after, then more").as_str())
            .exists());
        // ⌘Z puts it back.
        restore(&undone, &at.store, Some(&trash)).expect("restore");
        assert_eq!(at.read(), "after, then more");
        // A deleted file comes back, with nothing to trash; ⌘Z deletes it again.
        fs::remove_file(at.file()).expect("delete");
        let undone = undo_file(at.folder.path(), &change, &at.store, Some(&trash)).expect("undo");
        assert_eq!(at.read(), "before");
        assert_eq!(at.bin.took.borrow().len(), 1);
        restore(&undone, &at.store, Some(&trash)).expect("restore");
        assert!(!at.file().exists());
    }

    #[test]
    fn undoing_an_added_file_moves_it_to_the_trash_and_command_z_brings_it_back() {
        let at = setup();
        let trash = |path: &Path| at.bin.trash(path);
        fs::write(at.file(), "new").expect("write");
        let change = FileChange {
            path: "a.txt".into(),
            before: None,
            after: kept(&at.store, "new"),
        };
        assert!(matches!(
            undo_file(at.folder.path(), &change, &at.store, None),
            Err(ActionError::NoTrash)
        ));
        let undone = undo_file(at.folder.path(), &change, &at.store, Some(&trash)).expect("undo");
        assert_eq!(*at.bin.took.borrow(), ["new"]);
        assert!(!at.file().exists());
        restore(&undone, &at.store, Some(&trash)).expect("restore");
        assert_eq!(at.read(), "new");
    }

    #[test]
    fn undoing_a_hunk_puts_back_only_its_lines() {
        let at = setup();
        let trash = |path: &Path| at.bin.trash(path);
        let old: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let new = old
            .replacen("2\n", "two\n", 1)
            .replacen("19\n", "nineteen\n", 1);
        fs::write(at.file(), &new).expect("write");
        let change = FileChange {
            path: "a.txt".into(),
            before: kept(&at.store, &old),
            after: kept(&at.store, &new),
        };
        let clock = FixedClock::at(0);
        let shown = crate::changes::view::show(&at.store, &change, &clock).expect("shown");
        let hunk = &shown.diff.hunks[1];
        let undone =
            undo_hunk(at.folder.path(), &change, hunk, &at.store, Some(&trash)).expect("undo");
        assert_eq!(at.read(), old.replacen("2\n", "two\n", 1));
        assert_eq!(*at.bin.took.borrow(), std::slice::from_ref(&new));
        // ⌘Z right after puts it back; after another change it refuses.
        restore(&undone, &at.store, Some(&trash)).expect("restore");
        assert_eq!(at.read(), new);
        assert!(matches!(
            restore(&undone, &at.store, Some(&trash)),
            Err(ActionError::ChangedSince)
        ));
        // A file that no longer holds the hunk is left as it is.
        fs::write(at.file(), "other\n").expect("write");
        assert!(matches!(
            undo_hunk(at.folder.path(), &change, hunk, &at.store, Some(&trash)),
            Err(ActionError::ContextChanged)
        ));
        assert_eq!(at.read(), "other\n");
        assert!(ActionError::ContextChanged.say("en").contains("changed"));
    }

    #[test]
    fn a_file_whose_earlier_contents_were_not_kept_is_not_undone() {
        let at = setup();
        let change = FileChange {
            path: "big.bin".into(),
            before: Some(Entry {
                stamp: Stamp {
                    bytes: 1,
                    modified_ns: 0,
                },
                content: Content::TooLarge,
            }),
            after: None,
        };
        assert!(matches!(
            undo_file(at.folder.path(), &change, &at.store, None),
            Err(ActionError::NotKept)
        ));
        assert!(ActionError::NotKept.say("en").contains("kept"));
    }
}
