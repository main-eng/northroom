//! The folder whose changes a terminal feeds, and what git says differs there. A root is the
//! nearest folder holding `.git` (a folder, or the file a worktree has), else the drive, so
//! terminals anywhere in one repository share one Changes tab.
//!
//! The Changes tab's git scopes (`view::Scope`) compare the root with the last commit
//! (Uncommitted) and with where the branch left the default branch (vs main). They are read
//! through the person's own `git` with read-only commands and no optional locks, so nothing is
//! ever written to the repository (ADR-0002). Without git, or outside a repository, there are
//! none.

use crate::changes::scan::Stamp;
use crate::changes::snapshot::{self, Content, Entry, Manifest};
use crate::changes::store::Store;
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What marks a repository's top folder: a folder, or a file in a worktree.
pub const GIT_MARK: &str = ".git";
/// The default branch, tried in this order: the remote's own choice, then the usual names.
const ORIGIN_HEAD: &str = "refs/remotes/origin/HEAD";
const ORIGIN_PREFIX: &str = "refs/remotes/origin/";
const LOCAL_DEFAULTS: [&str; 2] = ["main", "master"];
/// Variables that would point git at another repository than the root's.
const REDIRECTING: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
];
/// The most file versions read from git kept in memory before starting over.
const MOST_KEPT_VERSIONS: usize = 10_000;

/// The root of changes made in `folder`: the nearest folder at or above it holding `.git`, not
/// above the drive, else the drive. A folder outside the drive is its own root.
pub fn root_for(folder: &Path, drive: &Path) -> PathBuf {
    let inside = folder.starts_with(drive);
    let repository = folder
        .ancestors()
        .take_while(|above| !inside || above.starts_with(drive))
        .find(|above| is_repository(above));
    match (repository, inside) {
        (Some(found), _) => found.to_path_buf(),
        (None, true) => drive.to_path_buf(),
        (None, false) => folder.to_path_buf(),
    }
}

/// Whether `folder` is a repository's top folder.
fn is_repository(folder: &Path) -> bool {
    folder.join(GIT_MARK).symlink_metadata().is_ok()
}

/// How a file differs from a commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// New since, tracked or not.
    Added,
    Deleted,
    Modified,
}

/// The default branch: its name as the person knows it (`main`) and the reference compared with
/// (`refs/remotes/origin/main`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefaultBranch {
    pub name: String,
    pub reference: String,
}

/// The commits a root's git scopes compare with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Revisions {
    /// The last commit; `None` before the first.
    pub head: Option<String>,
    /// The default branch, when there is one and the current branch is not it.
    pub branch: Option<DefaultBranch>,
    /// Where the current branch left it.
    pub branch_base: Option<String>,
}

/// The `git` program.
#[derive(Clone, Debug)]
pub struct Git {
    program: PathBuf,
}

impl Git {
    /// git on the `PATH`. On a Mac, `/usr/bin/git` only stands in for the developer tools and
    /// asks to install them, so it counts only once they are installed.
    pub fn find() -> Option<Self> {
        let path = std::env::var_os("PATH")?;
        let program = std::env::split_paths(&path)
            .map(|folder| folder.join("git"))
            .find(|candidate| candidate.is_file())?;
        if cfg!(target_os = "macos") && program == Path::new("/usr/bin/git") && !developer_tools() {
            return None;
        }
        Some(Self { program })
    }

    /// Runs `git -C root <arguments>` with no optional locks. Its output, or `None` when git
    /// failed (not a repository, an unknown revision).
    fn run(&self, root: &Path, arguments: &[&str]) -> io::Result<Option<Vec<u8>>> {
        let mut command = Command::new(&self.program);
        command
            .arg("-C")
            .arg(root)
            .args(arguments)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        for name in REDIRECTING {
            command.env_remove(name);
        }
        let output = command.output()?;
        Ok(output.status.success().then_some(output.stdout))
    }

    /// What git printed for `arguments`, trimmed; `None` when it failed or printed nothing.
    fn line(&self, root: &Path, arguments: &[&str]) -> io::Result<Option<String>> {
        let printed = self.run(root, arguments)?;
        Ok(printed
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .filter(|line| !line.is_empty()))
    }

    /// Whether `root` is in a repository's working tree.
    pub fn is_work_tree(&self, root: &Path) -> io::Result<bool> {
        let said = self.line(root, &["rev-parse", "--is-inside-work-tree"])?;
        Ok(said.as_deref() == Some("true"))
    }

    /// The commit `name` names, if any.
    fn commit(&self, root: &Path, name: &str) -> io::Result<Option<String>> {
        let asked = format!("{name}^{{commit}}");
        self.line(root, &["rev-parse", "--verify", "-q", &asked])
    }

    /// The default branch: the one `origin/HEAD` points at, else `main`, else `master`.
    pub fn default_branch(&self, root: &Path) -> io::Result<Option<DefaultBranch>> {
        let origin = self.line(root, &["symbolic-ref", "-q", ORIGIN_HEAD])?;
        if let Some(reference) = origin {
            if let Some(name) = reference.strip_prefix(ORIGIN_PREFIX) {
                if self.commit(root, &reference)?.is_some() {
                    return Ok(Some(DefaultBranch {
                        name: name.to_owned(),
                        reference,
                    }));
                }
            }
        }
        for name in LOCAL_DEFAULTS {
            let reference = format!("refs/heads/{name}");
            if self.commit(root, &reference)?.is_some() {
                return Ok(Some(DefaultBranch {
                    name: name.to_owned(),
                    reference,
                }));
            }
        }
        Ok(None)
    }

    /// The commits the scopes compare with: the last commit, and where the current branch left
    /// the default branch, unless it is the default branch.
    pub fn revisions(&self, root: &Path) -> io::Result<Revisions> {
        let Some(head) = self.commit(root, "HEAD")? else {
            return Ok(Revisions::default());
        };
        let current = self.line(root, &["symbolic-ref", "-q", "--short", "HEAD"])?;
        let branch = self
            .default_branch(root)?
            .filter(|branch| current.as_deref() != Some(branch.name.as_str()));
        let branch_base = match &branch {
            Some(branch) => self.line(root, &["merge-base", &head, &branch.reference])?,
            None => None,
        };
        Ok(Revisions {
            head: Some(head),
            branch: branch.filter(|_| branch_base.is_some()),
            branch_base,
        })
    }

    /// The files under `root` whose working tree differs from commit `base`, untracked ones
    /// (not ignored) as added; paths relative to `root`.
    pub fn changed(&self, root: &Path, base: &str) -> io::Result<Option<Vec<(String, Change)>>> {
        let diff = [
            "diff",
            "--name-status",
            "--no-renames",
            "--relative",
            "-z",
            base,
            "--",
        ];
        let Some(listed) = self.run(root, &diff)? else {
            return Ok(None);
        };
        let Some(new) = self.untracked(root)? else {
            return Ok(None);
        };
        let mut changed = parse_name_status(&listed);
        changed.extend(new.into_iter().map(|path| (path, Change::Added)));
        Ok(Some(changed))
    }

    /// The files under `root` git does not track and does not ignore.
    fn untracked(&self, root: &Path) -> io::Result<Option<Vec<String>>> {
        let asked = ["ls-files", "--others", "--exclude-standard", "-z"];
        Ok(self
            .run(root, &asked)?
            .map(|listed| split_nul(&listed).collect()))
    }

    /// File `path` (relative to `root`) as commit `revision` has it.
    fn show(&self, root: &Path, revision: &str, path: &str) -> io::Result<Option<Vec<u8>>> {
        self.run(root, &["show", &format!("{revision}:./{path}")])
    }
}

/// Whether the Mac's developer tools are installed, asked without offering to install them.
fn developer_tools() -> bool {
    Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// What `git diff --name-status -z` printed: each status, then its path.
fn parse_name_status(listed: &[u8]) -> Vec<(String, Change)> {
    let fields: Vec<String> = split_nul(listed).collect();
    fields
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[status, path]| {
            let change = match status.chars().next() {
                Some('A') => Change::Added,
                Some('D') => Change::Deleted,
                _ => Change::Modified,
            };
            (path.clone(), change)
        })
        .collect()
}

fn split_nul(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|&byte| byte == 0)
        .filter(|field| !field.is_empty())
        .map(|field| String::from_utf8_lossy(field).into_owned())
}

/// Each file that differs from a commit, by path, with the entry the commit has (`None` where
/// the commit has no such file).
pub type Committed = BTreeMap<String, Option<Entry>>;

/// What git says of a root: what differs from the last commit, and from where the branch left
/// the default branch when the current branch is not it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repository {
    pub uncommitted: Committed,
    pub branch: Option<(DefaultBranch, Committed)>,
}

/// Reads files from git into the store, keeping what each commit's file read as, since a
/// commit never changes.
pub struct Reader<'a> {
    pub git: &'a Git,
    pub store: &'a Store,
    pub kept: &'a mut HashMap<(String, String), Option<Entry>>,
}

impl Reader<'_> {
    /// What git says of `root`, whose latest snapshot is `latest`; `None` outside a repository.
    /// `leaves_out` says which paths the changes leave out (skipped and ignored), so git's view
    /// matches the snapshot's.
    pub fn repository(
        &mut self,
        root: &Path,
        latest: &Manifest,
        leaves_out: &dyn Fn(&str) -> bool,
    ) -> io::Result<Option<Repository>> {
        if !self.git.is_work_tree(root)? {
            return Ok(None);
        }
        let revisions = self.git.revisions(root)?;
        let uncommitted = match &revisions.head {
            Some(head) => self.committed(root, head, latest, leaves_out)?,
            // Nothing committed yet: every file is new.
            None => Some(
                latest
                    .files
                    .keys()
                    .map(|path| (path.clone(), None))
                    .collect(),
            ),
        };
        let Some(uncommitted) = uncommitted else {
            return Ok(None);
        };
        let branch = match (revisions.branch, &revisions.branch_base) {
            (Some(branch), Some(base)) => self
                .committed(root, base, latest, leaves_out)?
                .map(|committed| (branch, committed)),
            _ => None,
        };
        Ok(Some(Repository {
            uncommitted,
            branch,
        }))
    }

    /// Each file of `latest` that differs from `revision`, with the entry the commit has.
    fn committed(
        &mut self,
        root: &Path,
        revision: &str,
        latest: &Manifest,
        leaves_out: &dyn Fn(&str) -> bool,
    ) -> io::Result<Option<Committed>> {
        let Some(changed) = self.git.changed(root, revision)? else {
            return Ok(None);
        };
        let mut committed = Committed::new();
        for (path, change) in changed {
            if leaves_out(&path) {
                continue;
            }
            let entry = match change {
                Change::Added => None,
                // A changed file the snapshot did not list (not read in time) is left out.
                Change::Modified if !latest.files.contains_key(&path) => continue,
                Change::Modified | Change::Deleted => self.version(root, revision, &path)?,
            };
            committed.insert(path, entry);
        }
        Ok(Some(committed))
    }

    /// File `path` at `revision`, kept in the store; `None` when git has no such file.
    fn version(&mut self, root: &Path, revision: &str, path: &str) -> io::Result<Option<Entry>> {
        let key = (revision.to_owned(), path.to_owned());
        if let Some(entry) = self.kept.get(&key) {
            return Ok(entry.clone());
        }
        let entry = match self.git.show(root, revision, path)? {
            Some(bytes) => Some(entry_of(&bytes, self.store)?),
            None => None,
        };
        if self.kept.len() >= MOST_KEPT_VERSIONS {
            self.kept.clear();
        }
        self.kept.insert(key, entry.clone());
        Ok(entry)
    }
}

/// An entry for contents read from git: kept in the store unless too large.
fn entry_of(bytes: &[u8], store: &Store) -> io::Result<Entry> {
    let size = bytes.len() as u64;
    let content = if size > snapshot::LARGEST_BYTES {
        Content::TooLarge
    } else if snapshot::is_binary(bytes) {
        Content::Binary(store.put(bytes)?)
    } else {
        Content::Text(store.put(bytes)?)
    };
    Ok(Entry {
        stamp: Stamp {
            bytes: size,
            modified_ns: 0,
        },
        content,
    })
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Runs git in `folder` for a test, or `None` when git is not installed.
    pub fn git_in(folder: &Path, arguments: &[&str]) -> Option<()> {
        let status = Command::new("git")
            .arg("-C")
            .arg(folder)
            .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(arguments)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .ok()?;
        assert!(status.success(), "git {arguments:?} failed");
        Some(())
    }

    /// Writes `text` at `path` in `root`, making its folders.
    pub fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("folders");
        std::fs::write(path, text).expect("write");
    }

    /// A repository in `root` with `files` committed on `main`, and git to read it; `None` when
    /// git is not usable here.
    pub fn repository(root: &Path, files: &[(&str, &str)]) -> Option<Git> {
        for (path, text) in files {
            write(root, path, text);
        }
        git_in(root, &["init", "-q"])?;
        let git = Git::find()?;
        git_in(root, &["add", "-A"]);
        git_in(root, &["commit", "-q", "-m", "first"]);
        Some(git)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{git_in, repository, write};
    use super::*;
    use crate::changes::store::Hash;
    use std::fs;

    #[test]
    fn a_root_is_the_nearest_repository_else_the_drive() {
        let drive = tempfile::tempdir().expect("drive");
        let drive = drive.path();
        let made = |path: &str| {
            let folder = drive.join(path);
            fs::create_dir_all(&folder).expect("folders");
            folder
        };
        let repository = made("work/app");
        made("work/app/.git");
        let deep = made("work/app/src/ui");
        let docs = made("work/app/docs");
        let plain = made("notes/drafts");
        // A worktree marks its top folder with a `.git` file.
        let worktree = made("work/app-feature");
        fs::write(
            worktree.join(GIT_MARK),
            "gitdir: ../app/.git/worktrees/feature\n",
        )
        .expect("file");
        let inside_worktree = made("work/app-feature/lib");
        assert_eq!(root_for(&deep, drive), repository);
        assert_eq!(root_for(&docs, drive), repository);
        assert_eq!(root_for(&repository, drive), repository);
        assert_eq!(root_for(&inside_worktree, drive), worktree);
        assert_eq!(root_for(&plain, drive), drive);
        // A repository above the drive does not count.
        let above = tempfile::tempdir().expect("above");
        fs::create_dir_all(above.path().join(".git")).expect("git");
        let inner_drive = made_in(above.path(), "drive");
        let folder = made_in(&inner_drive, "a/b");
        assert_eq!(root_for(&folder, &inner_drive), inner_drive);
    }

    fn made_in(base: &Path, path: &str) -> PathBuf {
        let folder = base.join(path);
        fs::create_dir_all(&folder).expect("folders");
        folder
    }

    #[test]
    fn name_status_reads_each_status_and_path() {
        let listed = b"M\0src/a.rs\0A\0new file.txt\0D\0gone.md\0T\0link\0";
        assert_eq!(
            parse_name_status(listed),
            [
                ("src/a.rs".to_owned(), Change::Modified),
                ("new file.txt".to_owned(), Change::Added),
                ("gone.md".to_owned(), Change::Deleted),
                ("link".to_owned(), Change::Modified),
            ]
        );
    }

    #[test]
    fn the_default_branch_is_origin_s_choice_else_main_else_master() {
        let folder = tempfile::tempdir().expect("f");
        let root = folder.path();
        let Some(git) = repository(root, &[("a.txt", "a\n")]) else {
            eprintln!("git is not usable here; skipped");
            return;
        };
        let named = |name: &str, reference: &str| DefaultBranch {
            name: name.to_owned(),
            reference: reference.to_owned(),
        };
        assert_eq!(
            git.default_branch(root).expect("read"),
            Some(named("main", "refs/heads/main"))
        );
        // On the default branch there is no branch scope.
        assert_eq!(git.revisions(root).expect("read").branch, None);
        // Only master.
        git_in(root, &["branch", "-q", "-m", "main", "master"]);
        assert_eq!(
            git.default_branch(root).expect("read"),
            Some(named("master", "refs/heads/master"))
        );
        // Neither: none.
        git_in(root, &["branch", "-q", "-m", "master", "work"]);
        assert_eq!(git.default_branch(root).expect("read"), None);
        assert_eq!(git.revisions(root).expect("read").branch, None);
        // The remote's own choice wins over a local main.
        git_in(root, &["branch", "-q", "main"]);
        git_in(root, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
        git_in(
            root,
            &["symbolic-ref", ORIGIN_HEAD, "refs/remotes/origin/trunk"],
        );
        assert_eq!(
            git.default_branch(root).expect("read"),
            Some(named("trunk", "refs/remotes/origin/trunk"))
        );
        // Off it, on a branch of its own, the branch scope compares with it.
        let revisions = git.revisions(root).expect("read");
        assert_eq!(
            revisions.branch.map(|branch| branch.name).as_deref(),
            Some("trunk")
        );
        assert_eq!(revisions.branch_base, revisions.head);
        // A folder that is not a repository is no work tree.
        let plain = tempfile::tempdir().expect("plain");
        assert!(!git.is_work_tree(plain.path()).expect("ran"));
    }

    fn manifest_of(files: &[(&str, &str)]) -> Manifest {
        let mut manifest = Manifest::default();
        for (path, text) in files {
            let entry = Entry {
                stamp: Stamp {
                    bytes: text.len() as u64,
                    modified_ns: 1,
                },
                content: Content::Text(Hash::of(text.as_bytes())),
            };
            manifest.files.insert((*path).to_owned(), entry);
        }
        manifest
    }

    #[test]
    fn uncommitted_is_against_the_last_commit_and_the_branch_against_where_it_left_main() {
        let (folder, data) = (
            tempfile::tempdir().expect("f"),
            tempfile::tempdir().expect("d"),
        );
        let root = folder.path();
        let files = [
            ("src/a.rs", "fn a() {}\n"),
            ("docs/b.md", "b\n"),
            ("old.txt", "old\n"),
            (".gitignore", "build/\n"),
        ];
        let Some(git) = repository(root, &files) else {
            eprintln!("git is not usable here; skipped");
            return;
        };
        git_in(root, &["checkout", "-q", "-b", "feature"]);
        write(root, "src/a.rs", "fn a() { 1 }\n");
        git_in(root, &["commit", "-q", "-am", "on the branch"]);
        // Since the commit: one edit, one staged edit, one untracked file, one deletion, and
        // an ignored file.
        write(root, "docs/b.md", "b!\n");
        write(root, "staged.txt", "staged\n");
        git_in(root, &["add", "staged.txt"]);
        write(root, "new.txt", "new\n");
        write(root, "build/out.js", "built\n");
        fs::remove_file(root.join("old.txt")).expect("remove");
        let latest = manifest_of(&[
            ("src/a.rs", "fn a() { 1 }\n"),
            ("docs/b.md", "b!\n"),
            ("new.txt", "new\n"),
            ("staged.txt", "staged\n"),
            (".gitignore", "build/\n"),
        ]);
        let store = Store::at(data.path());
        let mut kept = HashMap::new();
        let mut reader = Reader {
            git: &git,
            store: &store,
            kept: &mut kept,
        };
        let repository = reader
            .repository(root, &latest, &|_| false)
            .expect("read")
            .expect("a repository");
        let paths = |committed: &Committed| committed.keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            paths(&repository.uncommitted),
            ["docs/b.md", "new.txt", "old.txt", "staged.txt"]
        );
        let b = repository.uncommitted["docs/b.md"].as_ref().expect("b");
        assert_eq!(b.content, Content::Text(Hash::of(b"b\n")));
        assert_eq!(repository.uncommitted["new.txt"], None);
        // vs main also counts the branch's own commit.
        let (branch, committed) = repository.branch.expect("a branch scope");
        assert_eq!(branch.name, "main");
        assert_eq!(
            paths(&committed),
            ["docs/b.md", "new.txt", "old.txt", "src/a.rs", "staged.txt"]
        );
        assert!(store.holds(&Hash::of(b"fn a() {}\n")));
        // Nothing was written to the repository.
        assert!(!root.join(".git/index.lock").exists());
        // A folder that is not a repository has no git scopes.
        let plain = tempfile::tempdir().expect("plain");
        let read = reader.repository(plain.path(), &latest, &|_| false);
        assert_eq!(read.expect("ran"), None);
    }
}
