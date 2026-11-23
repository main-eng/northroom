//! The installed app off a Mac. A Mac bundle keeps its version and identifier in `Info.plist`;
//! a Windows or Linux package keeps them in `Resources/release.json` beside the program, written
//! by `scripts/package-windows.sh` and `scripts/package-linux.sh`. A build-folder run has none
//! and does not update itself, like a Mac build outside a bundle.
//!
//! An update replaces the install folder on Windows (the per-user setup installs over it) and
//! the AppImage file on Linux (`$APPIMAGE`, set by the AppImage runtime).

use std::path::{Path, PathBuf};

/// The contents of `Resources/release.json`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Release {
    pub version: String,
    pub identifier: String,
}

/// The package description beside `program`, if any.
pub fn release_beside(program: &Path) -> Option<Release> {
    let file = program.parent()?.join("Resources").join("release.json");
    let text = std::fs::read_to_string(file).ok()?;
    let release: Release = serde_json::from_str(&text).ok()?;
    (!release.version.is_empty() && !release.identifier.is_empty()).then_some(release)
}

/// The running app's version, the path an update replaces, and its identifier; empty strings
/// when it is not an installed package.
pub fn running() -> (String, String, String) {
    // Use the path as the system gives it: `/proc/self/exe` on Linux, and not canonicalised on
    // Windows, where the setup's `/DIR=` rejects the `\\?\` form.
    let Ok(program) = std::env::current_exe() else {
        return Default::default();
    };
    let Some(release) = release_beside(&program) else {
        return Default::default();
    };
    match replaced(&program, std::env::var_os("APPIMAGE").map(PathBuf::from)) {
        Some(replaced) => (
            release.version,
            replaced.display().to_string(),
            release.identifier,
        ),
        None => Default::default(),
    }
}

/// What an update replaces: on Linux the AppImage `program` started from (`appimage`), if any;
/// elsewhere its folder.
fn replaced(program: &Path, appimage: Option<PathBuf>) -> Option<PathBuf> {
    if cfg!(target_os = "linux") {
        appimage.filter(|file| file.is_file())
    } else {
        program.parent().map(Path::to_path_buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_says_its_version_and_identifier_beside_its_program() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("Northroom.exe");
        assert_eq!(release_beside(&program), None, "a build has none");
        std::fs::create_dir_all(dir.path().join("Resources")).unwrap();
        let file = dir.path().join("Resources/release.json");
        std::fs::write(
            &file,
            r#"{"version":"0.3.1","identifier":"com.northroom.app"}"#,
        )
        .unwrap();
        assert_eq!(
            release_beside(&program),
            Some(Release {
                version: "0.3.1".into(),
                identifier: "com.northroom.app".into()
            })
        );
        std::fs::write(&file, r#"{"version":"","identifier":"com.northroom.app"}"#).unwrap();
        assert_eq!(release_beside(&program), None, "no version, no package");
    }

    #[test]
    fn an_update_replaces_the_folder_or_the_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("app/northroom");
        let appimage = dir.path().join("Northroom.AppImage");
        std::fs::write(&appimage, b"").unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!(replaced(&program, Some(appimage.clone())), Some(appimage));
            assert_eq!(
                replaced(&program, None),
                None,
                "an AppDir run as it is does not update"
            );
        } else {
            assert_eq!(replaced(&program, None), Some(dir.path().join("app")));
        }
    }
}
