//! App updates (ADR-0010): checking for, downloading, verifying and installing a new release.
//!
//! The feed is an electron-builder generic feed at the public repository's latest GitHub release
//! (`FEED`), read by electron-updater 6.8's rules (`feed`). The core owns the feed URL and
//! channel file, parsing, zip choice, the update decision, rollout, digest check, controller
//! and schedule, plus the files: download path, reuse of a complete zip, the temporary name,
//! cleanup on rejection, the unpack folder and the app found there. The shell does what needs
//! the system: sending requests (`transport`), unpacking (`ditto`), checking the code signature
//! against the running app's requirement, message boxes, and replacing and relaunching the app.
//!
//! A Mac reads `native-mac.yml`. The feed's `latest-mac.yml` is for a different build and is
//! never read.
//!
//! Windows and Linux follow the same steps with their own file and signature.
//! `native-windows.yml` lists the per-user setup (`Northroom-<version>-x64.exe`); the shell
//! checks its Authenticode signer against the running app's and its version against the offer,
//! then runs it silently. `native-linux.yml` lists the AppImage with a minisign signature, which
//! the core checks against the built-in key and the offered version (`signature`) before
//! replacing the running AppImage. Binding the version to the signature stops a tampered feed
//! from installing an older signed build. Off a Mac, the app's version and identity come from
//! `Resources/release.json` (`package`).

pub mod controller;
pub mod feed;
pub mod ffi;
pub mod package;
pub mod sha512;
pub mod signature;
pub mod version;

use crate::strings::{key, Message};
use crate::transport::{Reply, Request};
use controller::{Controller, Effect, Found, Origin, Words};
use std::path::{Path, PathBuf};

/// The feed URL. GitHub redirects `<FEED>/<name>` to that file in the newest release that is not
/// a draft or prerelease, and the transport follows the redirect.
pub const FEED: &str = "https://github.com/main-eng/northroom/releases/latest/download";
/// This app's channel. The feed's `latest` channel is not for this app.
pub const CHANNEL: &str = "native";
/// Check schedule: 30 s after start, then every 6 hours.
pub const FIRST_CHECK_SECONDS: f64 = 30.0;
pub const CHECK_INTERVAL_SECONDS: f64 = 6.0 * 60.0 * 60.0;

/// The update found and the file to download for it.
#[derive(Debug, Clone)]
pub struct Chosen {
    pub url: String,
    pub sha512: String,
    pub size: Option<u64>,
    pub version: String,
    /// The file's base64 minisign signature, when the channel file has one (Linux).
    pub signature: Option<String>,
}

/// The purpose of an updater request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The channel file.
    Fetch,
    /// The zip from `url`, written to `partial` and renamed to `zip` once complete.
    Download {
        url: String,
        zip: PathBuf,
        partial: PathBuf,
    },
}

/// A download's next step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Next {
    /// Send this request.
    Send(Step, Box<Request>),
    /// The file is complete and matches its digest. A Mac unpacks the zip next; elsewhere the
    /// signature is checked.
    Downloaded(PathBuf),
    Failed(Words),
}

/// The updater: the controller and its environment.
pub struct Updates {
    pub controller: Controller,
    feed: String,
    current_version: String,
    staging_id: String,
    os_release: String,
    platform: feed::Platform,
    arm64: bool,
    cache: PathBuf,
    pub chosen: Option<Chosen>,
    /// The unpacked, checked app waiting to be installed.
    pub staged: Option<PathBuf>,
}

/// Loopback host names.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

/// True for a plain `http://` loopback URL, with or without a port: the only feed override a
/// check may use. A check's local server speaks plain HTTP; an `https://` URL is never a local
/// check server.
fn is_local(address: &str) -> bool {
    let Some(rest) = address.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    LOOPBACK_HOSTS.iter().any(|host| {
        authority == *host
            || authority
                .strip_prefix(host)
                .and_then(|rest| rest.strip_prefix(':'))
                .is_some_and(|port| {
                    !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit())
                })
    })
}

impl Updates {
    /// `feed` overrides Northroom's feed only when it is a loopback URL (a check's local server).
    pub fn open(
        current_version: &str,
        supported: bool,
        support: &Path,
        cache: &Path,
        feed: Option<&str>,
    ) -> Updates {
        Updates {
            controller: Controller::new(current_version, supported),
            feed: feed
                .filter(|address| is_local(address))
                .unwrap_or(FEED)
                .to_owned(),
            current_version: current_version.to_owned(),
            staging_id: staging_id(support),
            os_release: os_release(),
            platform: feed::Platform::RUNNING,
            arm64: is_arm64(),
            cache: cache.to_owned(),
            chosen: None,
            staged: None,
        }
    }

    pub fn feed(&self) -> &str {
        &self.feed
    }

    pub fn feed_is_local(&self) -> bool {
        self.feed != FEED
    }

    pub fn staging_id(&self) -> &str {
        &self.staging_id
    }

    pub fn channel_file(&self) -> String {
        feed::channel_file(CHANNEL, self.platform)
    }

    pub fn channel_url(&self, now_ms: u64) -> String {
        feed::channel_url(&self.feed, &self.channel_file(), now_ms)
    }

    /// The folder for the update being downloaded.
    pub fn pending(&self) -> PathBuf {
        self.cache.join("pending")
    }

    /// The download path for the chosen zip: its file name in the pending folder.
    pub fn download_path(&self) -> Option<PathBuf> {
        let chosen = self.chosen.as_ref()?;
        let name = chosen
            .url
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("update.zip");
        let name = percent_decoded(name);
        // A server-supplied name cannot escape the folder.
        let name = Path::new(&name).file_name().map_or_else(
            || "update.zip".into(),
            |name| name.to_string_lossy().into_owned(),
        );
        Some(self.pending().join(name))
    }

    pub fn request(&mut self, origin: Origin) {
        self.controller.request(origin);
    }

    /// Handles the channel file reply. `status` is the HTTP status, or 0 with `failure` when
    /// nothing came back.
    pub fn fetched(&mut self, status: u32, body: &str, failure: &str) {
        let file = self.channel_file();
        let result = if status == 0 {
            Err(if failure.is_empty() {
                key::UPDATE_FEED_UNREACHABLE.into()
            } else {
                failure.into()
            })
        } else if status == 404 {
            Err(format!("Cannot find channel \"{file}\" update info: HttpError: 404").into())
        } else if !(200..300).contains(&status) {
            Err(format!("HttpError: {status}").into())
        } else {
            self.read(body).map_err(Words::from)
        };
        self.controller.fetched(result);
    }

    fn read(&mut self, body: &str) -> Result<Found, String> {
        let address = self.channel_url(0);
        let info = feed::parse_update_info(body, &self.channel_file(), &address)?;
        let available = feed::is_update_available(
            &info,
            &self.current_version,
            &self.os_release,
            &self.staging_id,
        )?;
        if available {
            let base = feed::base_url(&self.feed);
            let (url, file) = feed::choose_package(&info, &base, self.platform, self.arm64)?;
            self.chosen = Some(Chosen {
                url,
                sha512: file.sha512.clone(),
                size: file.size,
                version: info.version.clone(),
                signature: file.signature.clone(),
            });
        }
        Ok(Found {
            available,
            version: info.version,
        })
    }

    /// Checks a downloaded zip against the channel file's digest.
    pub fn check_download(&self, path: &Path) -> Result<(), String> {
        let chosen = self.chosen.as_ref().ok_or("Please check update first")?;
        sha512::check_file(path, &chosen.sha512)
    }

    pub fn next_effect(&mut self) -> Option<Effect> {
        self.controller.next_effect()
    }

    /// The channel file request, carrying the rollout id and bypassing caches.
    pub fn fetch_request(&self, now_ms: u64) -> Request {
        let headers = vec![
            ("x-user-staging-id", self.staging_id.clone()),
            ("Cache-Control", "no-cache".to_owned()),
        ];
        crate::transport::get(self.channel_url(now_ms), headers, None)
    }

    /// Starts the requested download. A complete zip is reused; otherwise the zip is written
    /// under a temporary name beside the final one, so a file under the final name is always
    /// complete.
    pub fn start_download(&self) -> Next {
        let (Some(chosen), Some(zip)) = (self.chosen.as_ref(), self.download_path()) else {
            return Next::Failed(key::UPDATE_NO_ZIP.into());
        };
        if let Some(folder) = zip.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if zip.is_file() && self.check_download(&zip).is_ok() {
            return Next::Downloaded(zip);
        }
        let partial = zip.with_file_name(format!(
            "temp-{}",
            zip.file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
        ));
        let _ = std::fs::remove_file(&partial);
        let request = crate::transport::get(
            chosen.url.clone(),
            vec![("Accept", "*/*".to_owned())],
            Some(partial.clone()),
        );
        Next::Send(
            Step::Download {
                url: chosen.url.clone(),
                zip,
                partial,
            },
            Box::new(request),
        )
    }

    /// Handles the zip reply: renames the zip into place and checks its digest, removing it on
    /// a mismatch.
    pub fn downloaded(&self, url: &str, zip: &Path, partial: &Path, reply: &Reply) -> Next {
        let refused = |reason: String| {
            let _ = std::fs::remove_file(partial);
            Next::Failed(reason.into())
        };
        if !reply.failure.is_empty() {
            return refused(reply.failure.clone());
        }
        if !(200..300).contains(&reply.status) {
            return refused(format!(
                "Cannot download \"{url}\", status {}",
                reply.status
            ));
        }
        let _ = std::fs::remove_file(zip);
        if let Err(error) = std::fs::rename(partial, zip) {
            return refused(error.to_string());
        }
        if let Err(refusal) = self.check_download(zip) {
            let _ = std::fs::remove_file(zip);
            return Next::Failed(refusal.into());
        }
        Next::Downloaded(zip.to_owned())
    }
}

/// An empty folder beside `zip` to unpack it into.
pub fn unpack_folder(zip: &Path) -> Result<PathBuf, String> {
    let folder = zip.with_file_name("unpacked");
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    Ok(folder)
}

/// The app `ditto` unpacked into `folder` with this app's bundle identifier (`identifier_of`
/// asks the platform). `exit` is ditto's status, `failure` why it could not run. The folder is
/// removed when no such app is found.
pub fn unpacked(
    folder: &Path,
    exit: i32,
    failure: &str,
    bundle_name: &str,
    identifier: &str,
    identifier_of: impl Fn(&Path) -> Option<String>,
) -> Result<PathBuf, Words> {
    let found = if !failure.is_empty() {
        Err(failure.into())
    } else if exit != 0 {
        Err(Message::new(key::UPDATE_NOT_UNPACKED)
            .with("exit", exit.to_string())
            .into())
    } else {
        let mut apps: Vec<PathBuf> = std::fs::read_dir(folder)
            .map_err(|error| Words::from(error.to_string()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|extension| extension == "app"))
            .collect();
        apps.sort();
        apps.into_iter()
            .find(|app| identifier_of(app).as_deref() == Some(identifier))
            .ok_or_else(|| {
                Message::new(key::UPDATE_NO_APP)
                    .with("bundle", bundle_name)
                    .into()
            })
    };
    if found.is_err() {
        let _ = std::fs::remove_dir_all(folder);
    }
    found
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if let Some(value) = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// This computer's staged-rollout id, stored as `.updaterId` in the app's support folder, the
/// name electron-updater uses.
fn staging_id(support: &Path) -> String {
    let file = support.join(".updaterId");
    if let Ok(text) = std::fs::read_to_string(&file) {
        if uuid::Uuid::parse_str(text.trim()).is_ok() && text.trim().len() == 36 {
            return text.trim().to_owned();
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    // If the id cannot be saved it still serves this session and is regenerated next time.
    let _ = std::fs::create_dir_all(support).and_then(|()| std::fs::write(&file, &id));
    id
}

/// The kernel release (`uname -r`, as node's `os.release()`), compared with an update's
/// `minimumSystemVersion`.
#[cfg(unix)]
fn os_release() -> String {
    // SAFETY: `uname` fills the struct; the release field is NUL-terminated.
    unsafe {
        let mut name: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut name) != 0 {
            return String::new();
        }
        std::ffi::CStr::from_ptr(name.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// Windows has no `uname`, and no Windows release sets `minimumSystemVersion`.
#[cfg(not(unix))]
fn os_release() -> String {
    String::new()
}

/// Off a Mac, the architecture the app was built for.
#[cfg(not(target_os = "macos"))]
fn is_arm64() -> bool {
    cfg!(target_arch = "aarch64")
}

/// Whether this is an Apple silicon Mac, native or under Rosetta (`sysctl.proc_translated`), as
/// MacUpdater decides it.
#[cfg(target_os = "macos")]
fn is_arm64() -> bool {
    if cfg!(target_arch = "aarch64") {
        return true;
    }
    let mut translated: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: the name is NUL-terminated and the value is an int of the given size.
    let found = unsafe {
        libc::sysctlbyname(
            c"sysctl.proc_translated".as_ptr(),
            (&raw mut translated).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    found == 0 && translated == 1
}

/// Replaces the running AppImage `running` with the verified `update`: copies it beside the
/// original under a hidden name, makes it executable and renames it over, so `running` is always
/// complete and the copy can cross file systems. The running app keeps its open file; the next
/// start uses the new one.
pub fn put_in_place(update: &Path, running: &Path) -> Result<(), String> {
    let name = running
        .file_name()
        .ok_or_else(|| format!("{} is not a file", running.display()))?;
    let beside = running.with_file_name(format!(".{}.update", name.to_string_lossy()));
    let placed = std::fs::copy(update, &beside)
        .and_then(|_| runnable(&beside))
        .and_then(|()| std::fs::rename(&beside, running));
    if let Err(error) = placed {
        let _ = std::fs::remove_file(&beside);
        return Err(error.to_string());
    }
    let _ = std::fs::remove_file(update);
    Ok(())
}

#[cfg(unix)]
fn runnable(file: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755))
}

/// Windows decides executability by file name.
#[cfg(not(unix))]
fn runnable(_: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_addresses_on_this_computer_are_local() {
        assert!(is_local("http://127.0.0.1:8123/feed"));
        assert!(is_local("http://localhost:9/"));
        assert!(is_local("http://localhost"));
        assert!(is_local("http://[::1]:3000"));
        assert!(!is_local("https://127.0.0.1:8123/"));
        assert!(!is_local("http://127.0.0.1.example.com/"));
        assert!(!is_local("http://example.com/?127.0.0.1"));
        assert!(!is_local("http://127.0.0.1:/"));
        assert!(!is_local("http://127.0.0.1:80a/"));
        assert!(!is_local("http://::1/"));
    }

    #[test]
    fn only_this_mac_replaces_the_feed() {
        let dir = tempfile::tempdir().unwrap();
        let updates = Updates::open(
            "0.0.1",
            true,
            dir.path(),
            dir.path(),
            Some("https://evil.example/feed"),
        );
        assert_eq!(updates.feed(), FEED);
        let updates = Updates::open(
            "0.0.1",
            true,
            dir.path(),
            dir.path(),
            Some("http://127.0.0.1:8123/feed"),
        );
        assert_eq!(updates.feed(), "http://127.0.0.1:8123/feed");
    }

    #[test]
    fn a_status_is_found_by_its_name() {
        use controller::Status;
        assert_eq!(Status::named("ready"), Some(Status::Ready));
        assert_eq!(Status::named("downloading"), Some(Status::Downloading));
        assert_eq!(Status::named("idle"), Some(Status::Idle));
        assert_eq!(Status::named("done"), None);
    }

    #[test]
    fn the_staging_id_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let first = staging_id(dir.path());
        assert_eq!(staging_id(dir.path()), first);
        assert_eq!(first.len(), 36);
    }

    fn found(dir: &Path, zip: &[u8]) -> Updates {
        let mut updates = Updates::open("0.0.1", true, dir, dir, None);
        let mut sha = sha512::Sha512::default();
        sha.update(zip);
        let digest = sha512::base64(&sha.finish());
        updates.chosen = Some(Chosen {
            url: "https://x.test/N%200.0.2.zip".into(),
            sha512: digest,
            size: None,
            version: "0.0.2".into(),
            signature: None,
        });
        updates
    }

    fn reply(status: u32, failure: &str) -> Reply {
        Reply {
            status,
            failure: failure.into(),
            ..Reply::default()
        }
    }

    #[test]
    fn a_download_is_written_under_a_temporary_name_and_renamed_once_whole() {
        let dir = tempfile::tempdir().unwrap();
        let updates = found(dir.path(), b"zip bytes");
        let Next::Send(Step::Download { url, zip, partial }, request) = updates.start_download()
        else {
            panic!("a request")
        };
        assert_eq!(
            (zip.clone(), partial.clone()),
            (
                dir.path().join("pending/N 0.0.2.zip"),
                dir.path().join("pending/temp-N 0.0.2.zip")
            )
        );
        assert_eq!(request.into.as_deref(), Some(partial.as_path()));
        assert_eq!(request.headers, [("Accept", "*/*".to_owned())]);
        std::fs::write(&partial, b"zip bytes").unwrap();
        assert_eq!(
            updates.downloaded(&url, &zip, &partial, &reply(200, "")),
            Next::Downloaded(zip.clone())
        );
        assert!(zip.is_file() && !partial.exists());
        // Already complete, so not downloaded again.
        assert_eq!(updates.start_download(), Next::Downloaded(zip));
    }

    #[test]
    fn a_refused_download_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let updates = found(dir.path(), b"zip bytes");
        let Next::Send(Step::Download { url, zip, partial }, _) = updates.start_download() else {
            panic!("a request")
        };
        assert_eq!(
            updates.downloaded(&url, &zip, &partial, &reply(404, "")),
            Next::Failed("Cannot download \"https://x.test/N%200.0.2.zip\", status 404".into())
        );
        std::fs::write(&partial, b"half").unwrap();
        assert_eq!(
            updates.downloaded(
                &url,
                &zip,
                &partial,
                &reply(200, "The network connection was lost.")
            ),
            Next::Failed("The network connection was lost.".into())
        );
        assert!(!partial.exists());
        std::fs::write(&partial, b"other bytes").unwrap();
        let Next::Failed(refusal) = updates.downloaded(&url, &zip, &partial, &reply(200, ""))
        else {
            panic!("refused")
        };
        assert!(
            refusal
                .to_string()
                .starts_with("sha512 checksum mismatch, expected "),
            "{refusal}"
        );
        assert!(!zip.exists() && !partial.exists());
        let mut none = found(dir.path(), b"");
        none.chosen = None;
        let Next::Failed(none) = none.start_download() else {
            panic!("refused")
        };
        assert_eq!(none.to_string(), "The update has no zip for this Mac.");
    }

    #[test]
    fn the_unpacked_app_is_the_one_with_this_apps_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("u.zip");
        let folder = unpack_folder(&zip).unwrap();
        assert_eq!(folder, dir.path().join("unpacked"));
        for name in ["Other.app", "Northroom.app", "notes.txt"] {
            std::fs::create_dir_all(folder.join(name)).unwrap();
        }
        let identify = |app: &Path| {
            Some(
                if app.ends_with("Northroom.app") {
                    "com.northroom.app"
                } else {
                    "com.other"
                }
                .to_owned(),
            )
        };
        assert_eq!(
            unpacked(
                &folder,
                0,
                "",
                "Northroom.app",
                "com.northroom.app",
                identify
            ),
            Ok(folder.join("Northroom.app"))
        );
        assert_eq!(
            unpacked(
                &folder,
                0,
                "",
                "Northroom.app",
                "com.northroom.dev",
                identify
            )
            .map_err(|reason| reason.to_string()),
            Err("The update holds no Northroom.app with this app's identifier.".to_owned())
        );
        assert!(!folder.exists(), "a folder with no such app is removed");
        let folder = unpack_folder(&zip).unwrap();
        assert_eq!(
            unpacked(&folder, 1, "", "Northroom.app", "x", identify)
                .map_err(|reason| reason.to_string()),
            Err("The update could not be unpacked (ditto exited with 1).".to_owned())
        );
        let folder = unpack_folder(&zip).unwrap();
        assert_eq!(
            unpacked(
                &folder,
                0,
                "ditto could not be run",
                "Northroom.app",
                "x",
                identify
            ),
            Err("ditto could not be run".into())
        );
    }

    #[test]
    fn an_update_is_put_in_place_whole_and_runnable() {
        let dir = tempfile::tempdir().unwrap();
        let running = dir.path().join("Applications/Northroom.AppImage");
        std::fs::create_dir_all(running.parent().unwrap()).unwrap();
        std::fs::write(&running, b"0.0.1").unwrap();
        let update = dir.path().join("pending/Northroom-0.0.2-x86_64.AppImage");
        std::fs::create_dir_all(update.parent().unwrap()).unwrap();
        std::fs::write(&update, b"0.0.2").unwrap();
        assert_eq!(put_in_place(&update, &running), Ok(()));
        assert_eq!(std::fs::read(&running).unwrap(), b"0.0.2");
        assert!(!update.exists(), "the download is used up");
        let left: Vec<_> = std::fs::read_dir(running.parent().unwrap())
            .unwrap()
            .collect();
        assert_eq!(left.len(), 1, "nothing is left beside it");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&running).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
        // A failed copy leaves the running file untouched.
        let missing = dir.path().join("pending/gone.AppImage");
        assert!(put_in_place(&missing, &running).is_err());
        assert_eq!(std::fs::read(&running).unwrap(), b"0.0.2");
    }

    #[test]
    fn a_download_keeps_its_name_inside_the_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut updates = Updates::open("0.0.1", true, dir.path(), dir.path(), None);
        updates.chosen = Some(Chosen {
            url: "https://x.test/a/..%2F..%2Fevil.zip".into(),
            sha512: String::new(),
            size: None,
            version: "1.0.0".into(),
            signature: None,
        });
        assert_eq!(
            updates.download_path().unwrap(),
            dir.path().join("pending/evil.zip")
        );
        updates.chosen = Some(Chosen {
            url: "https://x.test/Northroom%200.0.2.zip?x=1".into(),
            sha512: String::new(),
            size: None,
            version: "1.0.0".into(),
            signature: None,
        });
        assert_eq!(
            updates.download_path().unwrap(),
            dir.path().join("pending/Northroom 0.0.2.zip")
        );
    }
}
