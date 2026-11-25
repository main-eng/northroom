//! The folder whose changes a terminal feeds. A root is the nearest folder holding `.git` (a
//! folder, or the file a worktree has), else the drive, so terminals anywhere in one repository
//! share one Changes tab. Git itself is never run.

use std::path::{Path, PathBuf};

/// What marks a repository's top folder: a folder, or a file in a worktree.
pub const GIT_MARK: &str = ".git";

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
