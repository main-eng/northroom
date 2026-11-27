//! Which folder on this computer is the drive. The choice is the settings file's `drive`
//! (`file::DRIVE`), which the person or their agent may also edit. It is read at
//! launch. Choosing another folder never moves either folder's files.

use crate::settings::file;
use crate::strings::{self, key};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// No folder has been chosen yet.
    Unconfigured,
    Ready(PathBuf),
    /// The chosen folder cannot be used now: gone, moved, or on a drive that is not connected.
    Unavailable(PathBuf),
}

/// Whether `directory` is a real folder, at its own path, that can be read and written.
fn is_usable(directory: &Path) -> bool {
    let at_own_path = fs::canonicalize(directory).is_ok_and(|resolved| resolved == directory);
    let writable =
        fs::metadata(directory).is_ok_and(|stat| stat.is_dir() && !stat.permissions().readonly());
    at_own_path && writable && fs::read_dir(directory).is_ok()
}

/// The folder chosen on this device, as the settings file `file` records it. A leading `~`
/// means the home folder, since people write paths that way by hand.
pub fn read(file: &Path) -> Selection {
    let chosen = file::read(file)
        .ok()
        .and_then(|mut settings| settings.remove(file::DRIVE))
        .and_then(|drive| drive.as_str().map(expand_home))
        .filter(|directory| directory.is_absolute());
    match chosen {
        None => Selection::Unconfigured,
        Some(directory) if is_usable(&directory) => Selection::Ready(directory),
        Some(directory) => Selection::Unavailable(directory),
    }
}

fn expand_home(written: &str) -> PathBuf {
    let home = || std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match written.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with(['/', '\\']) => match home() {
            Some(home) => PathBuf::from(home).join(rest.trim_start_matches(['/', '\\'])),
            None => PathBuf::from(written),
        },
        _ => PathBuf::from(written),
    }
}

/// Makes `directory` the drive, stored by its canonical path so a symlink and its target are
/// the same choice. Fails, with a message in `language`, unless it is a readable and writable
/// folder.
pub fn choose(file: &Path, directory: &Path, language: &str) -> Result<PathBuf, String> {
    let resolved = fs::canonicalize(directory)
        .map_err(|_| strings::text(language, key::ONBOARDING_FOLDER_GONE).to_owned())?;
    if !is_usable(&resolved) {
        return Err(strings::text(language, key::ONBOARDING_FOLDER_UNUSABLE).to_owned());
    }
    file::write(
        file,
        file::DRIVE,
        serde_json::Value::String(resolved.to_string_lossy().into_owned()),
    )
    .map_err(|cause| cause.to_string())?;
    Ok(resolved)
}

/// Where the app keeps per-drive state, such as how the view was left: a folder under
/// `support` named from the drive's path, so two drives never share one.
pub fn metadata_directory(support: &Path, directory: &Path) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in directory.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    support.join("drives").join(format!("{hash:016x}"))
}

/// The first screen on a computer with no drive yet. Its words are the catalogs'
/// `onboarding_*` messages (`screen_layout::onboarding` places them).
pub mod onboarding {
    use super::Selection;
    use crate::strings::{self, key};

    /// The line under the button, in `language`: why the folder just picked was refused, or that
    /// the earlier choice is unavailable, or nothing.
    pub fn note<'a>(selection: &Selection, refusal: &'a str, language: &str) -> &'a str {
        match selection {
            _ if !refusal.is_empty() => refusal,
            Selection::Unavailable(_) | Selection::Ready(_) => {
                strings::text(language, key::ONBOARDING_UNAVAILABLE)
            }
            Selection::Unconfigured => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_screen_says_why_a_folder_is_not_the_drive() {
        let gone = Selection::Unavailable(PathBuf::from("/Volumes/Gone/Drive"));
        assert_eq!(onboarding::note(&Selection::Unconfigured, "", "en"), "");
        assert_eq!(
            onboarding::note(&gone, "", "en"),
            "This folder is no longer available. Reconnect its drive or choose another folder."
        );
        assert_eq!(
            onboarding::note(&gone, "Northroom cannot read and write this folder.", "en"),
            "Northroom cannot read and write this folder."
        );
        assert_eq!(
            onboarding::note(
                &Selection::Unconfigured,
                "This folder is no longer available.",
                "en"
            ),
            "This folder is no longer available."
        );
    }

    #[test]
    fn the_choice_is_kept_and_a_folder_that_has_gone_is_unavailable() {
        let support = tempfile::tempdir().expect("support");
        let settings = support.path().join("northroom").join("settings.json");
        let drive = tempfile::tempdir().expect("drive");
        assert_eq!(read(&settings), Selection::Unconfigured);

        let chosen = choose(&settings, drive.path(), "en").expect("choose");
        assert_eq!(read(&settings), Selection::Ready(chosen.clone()));
        assert!(choose(&settings, &drive.path().join("missing"), "en").is_err());
        assert_eq!(
            read(&settings),
            Selection::Ready(chosen.clone()),
            "a refused choice changes nothing"
        );

        let other = tempfile::tempdir().expect("other");
        assert_ne!(
            metadata_directory(support.path(), &chosen),
            metadata_directory(support.path(), other.path())
        );
        drop(drive);
        assert_eq!(read(&settings), Selection::Unavailable(chosen));
    }

    #[test]
    fn a_drive_written_by_hand_is_read_with_its_home_folder() {
        let folder = tempfile::tempdir().expect("folder");
        let settings = folder.path().join("settings.json");
        fs::write(&settings, "{ \"drive\": \"relative/path\" }").expect("relative");
        assert_eq!(
            read(&settings),
            Selection::Unconfigured,
            "only a whole path is a drive"
        );
        assert_eq!(
            expand_home("~/Northroom"),
            PathBuf::from(std::env::var_os("HOME").expect("home")).join("Northroom")
        );
        assert_eq!(expand_home("/a/~b"), PathBuf::from("/a/~b"));
        assert_eq!(expand_home("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn a_file_is_not_a_drive() {
        let support = tempfile::tempdir().expect("support");
        let file = support.path().join("notes.txt");
        fs::write(&file, b"x").expect("file");
        assert!(choose(&support.path().join("settings.json"), &file, "en").is_err());
    }
}
