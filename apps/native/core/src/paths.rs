//! Where the app keeps its own files on this computer, and where a check moves them. A shell
//! gives the platform's base folders once (`nr_paths_open`); the names under them are the core's.
//!
//! Renaming any of these would strand a person's settings and logs, so the tests pin each
//! resolved path.
//!
//! A check names its own folders: `NR_SUPPORT` moves the app's folder (and the logs and
//! downloads into it), `NR_WORKSPACE` the drive, and `NR_METADATA` what the app keeps for that
//! drive. A variable set to nothing counts as not set.

use crate::Relock;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The app's folder in each base.
const APP_FOLDER: &str = "Northroom Native";
/// What the app keeps for a check's drive when the check names none, in the base's app folder.
const CHECK_METADATA: &str = "Northroom Native/drive";
/// Inside a check's `NR_SUPPORT`.
const CHECK_LOGS: &str = "Logs";
const CHECK_CACHES: &str = "Caches";
/// The page host's compiled network block list: in the system's temporary folder (not the
/// person's Library), or inside a check's `NR_SUPPORT`.
const SCENE_HOST_TEMPORARY: &str = "northroom-scene-host";
const CHECK_SCENE_HOST: &str = "scene-host";
/// The folder Start on the first screen makes the drive, in the home folder.
/// The settings' folder, in the platform's folder for programs' settings.
const SETTINGS_FOLDER: &str = "northroom";
const SETTINGS_FOLDER_WINDOWS: &str = "Northroom";
/// Where change review kept its snapshots and records, in the app's folder. 1.4.0 was the last
/// version to use it (ADR-0002).
const CHANGE_REVIEW_FOLDER: &str = "changes";

/// The platform's folders the app's own go in: on a Mac `~/Library/Application Support`,
/// `~/Library/Logs` and `~/Library/Caches`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bases {
    pub application_support: PathBuf,
    pub logs: PathBuf,
    pub caches: PathBuf,
}

impl Bases {
    /// Where a Mac keeps them under `home`, for a process no shell starts (the agent's door).
    pub fn mac(home: &Path) -> Self {
        let library = home.join("Library");
        Self {
            application_support: library.join("Application Support"),
            logs: library.join("Logs"),
            caches: library.join("Caches"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// The app's own folder: what it remembers on its own, never the person's choices.
    pub support: PathBuf,
    /// The diagnostic log's folder.
    pub logs: PathBuf,
    /// Where update downloads are kept.
    pub caches: PathBuf,
    /// The person's settings, the one file they and their agent edit (`settings::file`): in the
    /// platform's folder for programs' settings (`~/.config/northroom` on a Mac and on Linux, as
    /// `XDG_CONFIG_HOME` says; `%APPDATA%\Northroom` on Windows), or a check's own.
    pub settings_file: PathBuf,
    /// The drive a check names (`NR_WORKSPACE`); the person's kept choice is then left alone.
    pub fixed_drive: Option<PathBuf>,
    /// What the app keeps for that drive.
    pub check_metadata: PathBuf,
    /// Where the page host keeps its compiled network block list.
    pub scene_host: PathBuf,
}

impl Paths {
    pub fn resolve(bases: &Bases, variable: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| {
            variable(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let check_support = set("NR_SUPPORT");
        let fixed_drive = set("NR_WORKSPACE");
        let check_metadata =
            set("NR_METADATA").unwrap_or_else(|| bases.application_support.join(CHECK_METADATA));
        Self {
            support: check_support
                .clone()
                .unwrap_or_else(|| bases.application_support.join(APP_FOLDER)),
            logs: check_support.as_ref().map_or_else(
                || bases.logs.join(APP_FOLDER),
                |support| support.join(CHECK_LOGS),
            ),
            caches: check_support.as_ref().map_or_else(
                || bases.caches.join(APP_FOLDER),
                |support| support.join(CHECK_CACHES),
            ),
            scene_host: check_support.as_ref().map_or_else(
                || std::env::temp_dir().join(SCENE_HOST_TEMPORARY),
                |support| support.join(CHECK_SCENE_HOST),
            ),
            check_metadata: check_metadata.clone(),
            settings_file: match (&fixed_drive, &check_support) {
                // A check keeps its settings with what it names, never in the person's.
                (Some(_), _) => check_metadata.join(crate::settings::file::NAME),
                (None, Some(support)) => support.join(crate::settings::file::NAME),
                (None, None) => settings_folder(&set).join(crate::settings::file::NAME),
            },
            fixed_drive,
        }
    }
}

/// The platform's folder for this program's settings, from the environment `set` reads.
fn settings_folder(set: &impl Fn(&str) -> Option<PathBuf>) -> PathBuf {
    if cfg!(target_os = "windows") {
        if let Some(roaming) = set("APPDATA") {
            return roaming.join(SETTINGS_FOLDER_WINDOWS);
        }
    }
    set("XDG_CONFIG_HOME")
        .or_else(|| set("HOME").map(|home| home.join(".config")))
        .or_else(|| set("USERPROFILE").map(|home| home.join(".config")))
        .unwrap_or_default()
        .join(SETTINGS_FOLDER)
}

/// Removes what change review left in the app's folder `support`: that one folder and nothing
/// beside it. A link in its place is removed, not followed, and no folder there is no failure.
/// This step can go once nobody still runs a version from before it.
pub fn remove_change_review_data(support: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(support.join(CHANGE_REVIEW_FOLDER)) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

static PATHS: Mutex<Option<Paths>> = Mutex::new(None);

/// Resolves the app's folders over the platform's `bases` for this process's environment.
pub fn open(bases: &Bases) {
    *PATHS.relock() = Some(Paths::resolve(bases, |name| std::env::var(name).ok()));
}

/// This process's folders, once a shell gave the bases; in a process no shell started (the
/// agent's door), the ones a Mac has under `HOME`.
pub fn get() -> Paths {
    let mut paths = PATHS.relock();
    paths
        .get_or_insert_with(|| {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default();
            Paths::resolve(&Bases::mac(&home), |name| std::env::var(name).ok())
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bases() -> Bases {
        Bases::mac(Path::new("/Users/someone"))
    }

    fn resolve(pairs: &[(&str, &str)]) -> Paths {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Paths::resolve(&bases(), move |name| {
            pairs
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.clone())
        })
    }

    /// The names a person's data lives under. Changing any of these loses it.
    #[test]
    fn a_persons_folders_keep_their_names() {
        let paths = resolve(&[("HOME", "/Users/someone")]);
        assert_eq!(
            paths.support,
            Path::new("/Users/someone/Library/Application Support/Northroom Native")
        );
        assert_eq!(
            paths.logs,
            Path::new("/Users/someone/Library/Logs/Northroom Native")
        );
        assert_eq!(
            paths.caches,
            Path::new("/Users/someone/Library/Caches/Northroom Native")
        );
        assert_eq!(paths.fixed_drive, None);
        assert_eq!(
            paths.settings_file,
            Path::new("/Users/someone/.config/northroom/settings.json")
        );
        assert_eq!(
            resolve(&[("HOME", "/Users/someone"), ("XDG_CONFIG_HOME", "/cfg")]).settings_file,
            Path::new("/cfg/northroom/settings.json")
        );
        assert_eq!(
            paths.check_metadata,
            Path::new("/Users/someone/Library/Application Support/Northroom Native/drive")
        );
        assert_eq!(
            paths.scene_host,
            std::env::temp_dir().join("northroom-scene-host")
        );
    }

    #[test]
    fn what_change_review_left_is_removed_and_nothing_beside_it() {
        let support = tempfile::tempdir().expect("support");
        let left = support.path().join("changes");
        std::fs::create_dir_all(left.join("objects/ab")).expect("store");
        std::fs::write(left.join("objects/ab/cd"), "kept once").expect("object");
        std::fs::write(left.join("records.json"), "{}").expect("record");
        std::fs::write(support.path().join("selection.json"), "{}").expect("beside");
        std::fs::create_dir(support.path().join("changes-of-mine")).expect("beside");
        remove_change_review_data(support.path()).expect("removed");
        assert!(!left.exists());
        assert!(support.path().join("selection.json").is_file());
        assert!(support.path().join("changes-of-mine").is_dir());
        // The next launch finds nothing to remove.
        remove_change_review_data(support.path()).expect("nothing there");
    }

    #[test]
    #[cfg(unix)]
    fn a_link_where_change_review_kept_its_data_is_removed_not_followed() {
        let (support, elsewhere) = (
            tempfile::tempdir().expect("support"),
            tempfile::tempdir().expect("elsewhere"),
        );
        std::fs::write(elsewhere.path().join("mine.txt"), "mine").expect("file");
        let link = support.path().join("changes");
        std::os::unix::fs::symlink(elsewhere.path(), &link).expect("link");
        remove_change_review_data(support.path()).expect("removed");
        assert!(link.symlink_metadata().is_err());
        assert!(elsewhere.path().join("mine.txt").is_file());
    }

    #[test]
    fn a_check_moves_the_apps_folder_and_its_logs_and_downloads_into_it() {
        let paths = resolve(&[("NR_SUPPORT", "/tmp/run/support")]);
        assert_eq!(paths.support, Path::new("/tmp/run/support"));
        assert_eq!(paths.logs, Path::new("/tmp/run/support/Logs"));
        assert_eq!(paths.caches, Path::new("/tmp/run/support/Caches"));
        assert_eq!(paths.scene_host, Path::new("/tmp/run/support/scene-host"));
        assert_eq!(
            paths.settings_file,
            Path::new("/tmp/run/support/settings.json"),
            "a check never reads or writes the person's settings"
        );
    }

    #[test]
    fn a_check_that_names_its_drive_keeps_its_settings_with_it() {
        let paths = resolve(&[
            ("NR_WORKSPACE", "/tmp/run/workspace"),
            ("NR_METADATA", "/tmp/run/metadata"),
        ]);
        assert_eq!(
            paths.fixed_drive.as_deref(),
            Some(Path::new("/tmp/run/workspace"))
        );
        assert_eq!(
            paths.settings_file,
            Path::new("/tmp/run/metadata/settings.json")
        );
        let unnamed = resolve(&[("NR_WORKSPACE", "/tmp/run/workspace")]);
        assert_eq!(
            unnamed.check_metadata,
            Path::new("/Users/someone/Library/Application Support/Northroom Native/drive")
        );
    }

    #[test]
    fn a_variable_set_to_nothing_is_not_set() {
        assert_eq!(
            resolve(&[
                ("NR_SUPPORT", ""),
                ("NR_WORKSPACE", ""),
                ("NR_METADATA", "")
            ]),
            resolve(&[])
        );
    }
}
