//! Walks a watched folder for its files: everything at any depth except tool and build folders
//! and what the folder's `.gitignore` files leave out. Symbolic links are not followed. A walk
//! stops after 200,000 entries or its time, and says which folders it listed in full.

use crate::changes::ignore::{Kind, Rules, IGNORE_FILE};
use crate::changes::Clock;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

/// Names never walked or watched: version control, packages, build output and Finder's files.
pub const SKIPPED_NAMES: [&str; 7] = [
    ".git",
    "node_modules",
    "target",
    "build",
    "dist",
    ".venv",
    ".DS_Store",
];

/// The most entries, files and folders, a walk reads before it stops.
pub const MOST_ENTRIES: usize = 200_000;

/// When a walk stops early: after so many entries, or once the clock passes `until_ms`.
pub struct Limit<'a> {
    pub most_entries: usize,
    pub clock: &'a dyn Clock,
    pub until_ms: u64,
}

impl Limit<'_> {
    fn reached(&self, entries: usize) -> bool {
        entries >= self.most_entries || self.clock.now_ms() > self.until_ms
    }
}

/// A file's size and modification time, which say whether it may have changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stamp {
    pub bytes: u64,
    pub modified_ns: u64,
}

/// One file found, by its path relative to the root with `/` between names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: String,
    pub stamp: Stamp,
}

/// What a walk found.
#[derive(Debug, Default)]
pub struct Walked {
    pub files: Vec<Found>,
    /// The ignore rules read on the way, for the watcher's events.
    pub rules: Rules,
    /// Folders that could not be read, left out.
    pub unreadable: usize,
    /// The walk stopped at its limit, so files in folders not in `listed` may be missing.
    pub partial: bool,
    /// The folders read in full ("" for the root), when the walk stopped early.
    pub listed: BTreeSet<String>,
    entries: usize,
}

/// Whether a path relative to the root goes through a skipped name.
pub fn skips(path: &str) -> bool {
    path.split('/').any(|name| SKIPPED_NAMES.contains(&name))
}

/// Walks `root` until `limit`. Fails only when the root itself cannot be read.
pub fn walk(root: &Path, limit: &Limit) -> io::Result<Walked> {
    let mut walked = Walked::default();
    let mut folders = vec![String::new()];
    fs::read_dir(root)?;
    while let Some(folder) = folders.pop() {
        if limit.reached(walked.entries) {
            walked.partial = true;
            break;
        }
        let path = if folder.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&folder)
        };
        match walk_folder(&path, &folder, &mut walked, limit) {
            Ok(found) => folders.extend(found),
            Err(_) => walked.unreadable += 1,
        }
    }
    walked.files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(walked)
}

/// Reads one folder (`relative` below the root): its rules, then its files into `walked`,
/// noting it as listed when read to the end within `limit`. Returns the folders in it still to
/// walk.
fn walk_folder(
    path: &Path,
    relative: &str,
    walked: &mut Walked,
    limit: &Limit,
) -> io::Result<Vec<String>> {
    if let Ok(text) = fs::read_to_string(path.join(IGNORE_FILE)) {
        walked.rules.add(relative, &text);
    }
    let mut folders = Vec::new();
    for entry in fs::read_dir(path)? {
        if walked.entries >= limit.most_entries {
            walked.partial = true;
            return Ok(folders);
        }
        walked.entries += 1;
        folders.extend(take_entry(&entry?, relative, walked)?);
    }
    walked.listed.insert(relative.to_owned());
    Ok(folders)
}

/// The folder holding `path` ("" for the root).
pub fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(folder, _)| folder)
}

/// Takes one entry of folder `relative`: a file into `walked`, or a folder to walk, unless it
/// is skipped or ignored.
fn take_entry(
    entry: &fs::DirEntry,
    relative: &str,
    walked: &mut Walked,
) -> io::Result<Option<String>> {
    let name = entry.file_name().to_string_lossy().into_owned();
    if SKIPPED_NAMES.contains(&name.as_str()) {
        return Ok(None);
    }
    let below = if relative.is_empty() {
        name
    } else {
        format!("{relative}/{name}")
    };
    let kind = entry.file_type()?;
    let (is_folder, is_file) = (kind.is_dir(), kind.is_file());
    let as_kind = if is_folder { Kind::Folder } else { Kind::File };
    if !(is_folder || is_file) || walked.rules.ignores_itself(&below, as_kind) {
        return Ok(None);
    }
    if is_folder {
        return Ok(Some(below));
    }
    let stamp = stamp_of(&entry.metadata()?);
    walked.files.push(Found { path: below, stamp });
    Ok(None)
}

pub fn stamp_of(meta: &fs::Metadata) -> Stamp {
    let modified_ns = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        });
    Stamp {
        bytes: meta.len(),
        modified_ns,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::testing::FixedClock;

    fn unlimited(clock: &FixedClock) -> Limit<'_> {
        Limit {
            most_entries: MOST_ENTRIES,
            clock,
            until_ms: u64::MAX,
        }
    }

    #[test]
    fn a_walk_skips_tool_folders_and_ignored_paths_at_any_depth() {
        let folder = tempfile::tempdir().expect("folder");
        let root = folder.path();
        let write = |path: &str, text: &str| {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("parent")).expect("folders");
            fs::write(path, text).expect("write");
        };
        write("readme.md", "hi");
        write("src/main.rs", "fn main() {}");
        write("node_modules/x/index.js", "x");
        write("src/target/debug.o", "x");
        write(".git/HEAD", "x");
        write(".DS_Store", "x");
        write(".gitignore", "*.log\nsecrets/\n");
        write("app.log", "x");
        write("secrets/key.txt", "x");
        write("src/.gitignore", "generated.rs\n!keep.log\n");
        write("src/generated.rs", "x");
        write("src/keep.log", "kept");
        write("other/generated.rs", "x");
        let clock = FixedClock::at(0);
        let walked = walk(root, &unlimited(&clock)).expect("walk");
        let paths: Vec<&str> = walked.files.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                ".gitignore",
                "other/generated.rs",
                "readme.md",
                "src/.gitignore",
                "src/keep.log",
                "src/main.rs"
            ]
        );
        assert_eq!(walked.files[2].stamp.bytes, 2);
        assert!(walked.rules.ignores("src/generated.rs", Kind::File));
        assert!(walked.rules.ignores("secrets/new.txt", Kind::File));
        assert!(skips("a/node_modules/b.js") && !skips("a/modules/b.js"));
        assert_eq!(walked.unreadable, 0);
        assert!(!walked.partial);
    }

    #[test]
    fn a_walk_stops_at_its_limit_and_says_which_folders_it_listed() {
        let folder = tempfile::tempdir().expect("folder");
        let root = folder.path();
        for name in ["a", "b", "c"] {
            fs::create_dir_all(root.join(name)).expect("folder");
            for file in 0..5 {
                fs::write(root.join(format!("{name}/{file}.txt")), "x").expect("write");
            }
        }
        let clock = FixedClock::at(0);
        let small = Limit {
            most_entries: 9,
            ..unlimited(&clock)
        };
        let walked = walk(root, &small).expect("walk");
        assert!(walked.partial);
        assert!(walked.files.len() < 15);
        // The root's three folders were listed; at most one of them in full.
        assert!(walked.listed.contains(""));
        assert!(walked.listed.len() <= 2);
        // A spent clock stops it before the first folder.
        let spent = Limit {
            until_ms: 0,
            clock: &FixedClock::at(1),
            most_entries: MOST_ENTRIES,
        };
        let stopped = walk(root, &spent).expect("walk");
        assert!(stopped.partial && stopped.files.is_empty());
        assert_eq!(folder_of("a/b/c.txt"), "a/b");
        assert_eq!(folder_of("c.txt"), "");
    }

    #[test]
    fn a_missing_root_fails() {
        let folder = tempfile::tempdir().expect("folder");
        let clock = FixedClock::at(0);
        assert!(walk(&folder.path().join("gone"), &unlimited(&clock)).is_err());
    }
}
