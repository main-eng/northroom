//! The updater's boundary. The shell opens it once at launch, requests checks, takes effects one
//! at a time and reports results; requests go through `transport`. Returned text is borrowed
//! UTF-8, valid until the next call here that returns text.
//!
//! A check with a local feed (`NR_UPDATE_FEED`, loopback only) may start the first check sooner
//! (`NR_UPDATE_FIRST_CHECK`, seconds) and set the required signature (`NR_UPDATE_REQUIREMENT`),
//! because an ad-hoc signed build's own requirement is its exact hash, which no other build
//! meets. It is a code requirement on a Mac, the base64 SHA-512 of the signer's public key on
//! Windows, and the minisign public key on Linux. In an unattended run, dialogs are answered
//! from `NR_UPDATE_ANSWERS` (comma-separated button indices), every effect is printed to stderr,
//! and a relaunch with `NR_DO=update:launched` prints its version and location.

use super::controller::{Effect, Origin, Status, Words};
use super::feed::Platform;
use super::{Next, Step, Updates};
use crate::ffi::{text_of, NrText};
use crate::strings::{self, key};
use crate::transport::{Reply, Request};
use crate::Relock;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{c_char, CString};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// Shell callback that reads the bundle identifier of the app at NUL-terminated `path`. It
/// writes UTF-8 into `out` (at most `capacity` bytes) and returns the length, 0 for none.
pub type NrBundleIdentifier =
    extern "C" fn(path: *const c_char, out: *mut u8, capacity: usize) -> usize;

/// The running app, as the shell describes it at launch.
struct Running {
    bundle: PathBuf,
    identifier: String,
    identifier_of: NrBundleIdentifier,
}

/// Why an install was started.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Installing {
    Relaunch,
    Quit,
}

struct State {
    updates: Updates,
    running: Running,
    /// Requests waiting for the transport.
    outgoing: VecDeque<(Step, Request)>,
    /// Shell effects queued besides the controller's.
    told: VecDeque<Told>,
    /// The requested install and the app being installed.
    installing: Option<(Installing, PathBuf)>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// A status override for the title bar button, for screenshots.
static PICTURED: Mutex<Option<Status>> = Mutex::new(None);
thread_local! {
    /// The text last lent on each thread. The shell reads some off the main thread (the
    /// signature requirement), so one thread's text is never freed while another copies its
    /// own.
    static LENT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn lend(text: String) -> NrText {
    LENT.with(|lent| {
        let mut lent = lent.borrow_mut();
        *lent = text;
        NrText::borrowing(&lent)
    })
}

fn with<T>(default: T, action: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.relock();
    state.as_mut().map_or(default, action)
}

fn with_updates<T>(default: T, action: impl FnOnce(&mut Updates) -> T) -> T {
    with(default, |state| action(&mut state.updates))
}

/// An environment variable from launch; an empty value counts as unset.
fn variable(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// An effect for the shell (`update_next_effect`). An unattended run prints each to stderr as
/// JSON, tagged by `kind`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Told {
    /// The status changed; the menu item, tray and title bar follow it.
    Status { name: &'static str },
    /// Show a dialog and report the button pressed (`nr_update_answered`).
    Show(UpdateBox),
    /// `ditto -x -k` the zip into the empty folder (`nr_update_unpacked`). Mac only.
    Unpack { zip: String, folder: String },
    /// Check the signature of the unpacked Mac bundle or the downloaded file (Windows setup,
    /// Linux AppImage) (`nr_update_verified`).
    Verify { app: String },
    /// Re-check it and replace this app, or on Windows run the setup that does
    /// (`nr_update_installed`).
    Install { app: String },
    /// Quit, then open the new app with `update_relaunch_environment`.
    Relaunch,
}

/// An updater dialog: whether it is an error, its text, the buttons in order, the Return button
/// when it is not the first, and the Escape button.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct UpdateBox {
    pub error: bool,
    pub message: String,
    pub detail: String,
    pub buttons: Vec<&'static str>,
    pub return_button: Option<usize>,
    pub escape_button: Option<usize>,
}

/// The platform's result for a code signature check (`nr_update_verified`) or an install
/// (`nr_update_installed`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrSignature {
    /// Intact, nested code included, and meets the requirement, so it may replace this app.
    Passed,
    /// The app's signature could not be read at all.
    Unreadable,
    /// This app's own requirement, which the update must meet, is unreadable.
    OwnUnreadable,
    /// The check failed; the system gives the reason.
    Failed,
    /// The check passed but replacing this app failed; the system gives the reason.
    NotReplaced,
}

/// Why `app` may not replace this one: the system's `reason`, else the `status` error code.
/// `None` when it may.
fn refusal(check: NrSignature, status: i32, reason: &str, app: &Path) -> Option<String> {
    let reason = if reason.is_empty() {
        format!("OSStatus {status}")
    } else {
        reason.to_owned()
    };
    let at = app.display();
    match check {
        NrSignature::Passed => None,
        NrSignature::Unreadable => Some(format!("Code signature at URL {at} could not be read.")),
        NrSignature::OwnUnreadable => {
            Some("This app's own code signature could not be read.".to_owned())
        }
        NrSignature::Failed => Some(format!(
            "Code signature at URL {at} did not pass validation: {reason}"
        )),
        NrSignature::NotReplaced => Some(reason),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Starts the updater with the running app's version, bundle and identifier, and the callback
/// that reads another bundle's identifier. Only an installed release with a version can replace
/// itself. The rollout id lives in the app's support folder and downloads in its caches
/// (`paths::Paths`).
///
/// # Safety
/// Each text is null or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_update_open(
    current_version: *const c_char,
    bundle: *const c_char,
    identifier: *const c_char,
    identifier_of: NrBundleIdentifier,
) {
    // SAFETY: as the caller promises.
    let (version, bundle, identifier) = unsafe {
        (
            text_of(current_version),
            text_of(bundle),
            text_of(identifier),
        )
    };
    let bundle = PathBuf::from(bundle);
    // Only an installed app updates itself: a Mac bundle, a Windows install folder or a Linux
    // AppImage with `Resources/release.json` (`package`). A build-folder run reports updates as
    // unsupported (`UPDATE_UNSUPPORTED`).
    let supported = !version.is_empty()
        && match Platform::RUNNING {
            Platform::Mac => bundle
                .extension()
                .is_some_and(|extension| extension == "app"),
            Platform::Windows | Platform::Linux => {
                !identifier.is_empty() && !bundle.as_os_str().is_empty()
            }
        };
    let feed = variable("NR_UPDATE_FEED");
    let paths = crate::paths::get();
    let updates = Updates::open(
        &version,
        supported,
        &paths.support,
        &paths.caches,
        feed.as_deref(),
    );
    if variable("NR_DO").as_deref() == Some("update:launched") {
        // A check's relaunched app prints its version and location.
        eprintln!(
            "update\tlaunched\t{version}\t{}\t{}",
            std::process::id(),
            bundle.display()
        );
    }
    let running = Running {
        bundle,
        identifier,
        identifier_of,
    };
    *STATE.relock() = Some(State {
        updates,
        running,
        outgoing: VecDeque::new(),
        told: VecDeque::new(),
        installing: None,
    });
}

/// Whether this app can update itself. Background checks run only then.
#[no_mangle]
pub extern "C" fn nr_update_supported() -> bool {
    with_updates(false, |updates| updates.controller.supported())
}

/// Seconds until the first background check after launch (`first`), or between checks.
#[no_mangle]
pub extern "C" fn nr_update_schedule(first: bool) -> f64 {
    if !first {
        return super::CHECK_INTERVAL_SECONDS;
    }
    let sooner = variable("NR_UPDATE_FIRST_CHECK").and_then(|seconds| seconds.parse::<f64>().ok());
    match (
        with_updates(false, |updates| updates.feed_is_local()),
        sooner,
    ) {
        (true, Some(seconds)) => seconds,
        _ => super::FIRST_CHECK_SECONDS,
    }
}

/// The signature an update must have, in platform terms (see the module docs), when a local
/// check sets one; empty to use the running app's own.
#[no_mangle]
pub extern "C" fn nr_update_requirement() -> NrText {
    let local = with_updates(false, |updates| updates.feed_is_local());
    lend(
        variable("NR_UPDATE_REQUIREMENT")
            .filter(|_| local)
            .unwrap_or_default(),
    )
}

/// Starts a check that presents what it finds: manual (menu, tray, title bar button) or
/// background (schedule).
#[no_mangle]
pub extern "C" fn nr_update_request(manual: bool) {
    with_updates((), |updates| {
        updates.request(if manual {
            Origin::Manual
        } else {
            Origin::Background
        })
    });
}

/// Carries out the controller's effect when the core owns it: queues a request, takes a file
/// step or logs a failure. Otherwise returns it for the shell, with dialogs in `language`.
fn take(state: &mut State, effect: Effect, language: &str) -> Option<Told> {
    match effect {
        Effect::Status(status) => Some(Told::Status {
            name: status.name(),
        }),
        Effect::Fetch => {
            state
                .outgoing
                .push_back((Step::Fetch, state.updates.fetch_request(now_ms())));
            None
        }
        Effect::Download => {
            let next = state.updates.start_download();
            follow(state, next);
            None
        }
        Effect::Show(dialog) => Some(Told::Show(UpdateBox {
            error: dialog.error,
            return_button: dialog.return_button(),
            escape_button: dialog.escape_button(),
            message: dialog.message.say(language),
            detail: dialog.detail.say(language),
            buttons: dialog
                .buttons
                .iter()
                .map(|button| strings::text(language, *button))
                .collect(),
        })),
        Effect::Relaunch => match state.updates.staged.clone() {
            Some(app) => {
                state.installing = Some((Installing::Relaunch, app.clone()));
                Some(Told::Install {
                    app: app.display().to_string(),
                })
            }
            None => {
                state
                    .updates
                    .controller
                    .install_failed(key::UPDATE_NOT_STAGED.into());
                None
            }
        },
        Effect::Log(reason) => {
            crate::diagnostics::write(
                crate::diagnostics::Level::Error,
                &format!("[updater] {reason}"),
            );
            None
        }
    }
}

/// Takes the download's next step.
fn follow(state: &mut State, next: Next) {
    match next {
        Next::Send(step, request) => state.outgoing.push_back((step, *request)),
        Next::Downloaded(zip) if Platform::RUNNING == Platform::Mac => {
            match super::unpack_folder(&zip) {
                Ok(folder) => state.told.push_back(Told::Unpack {
                    zip: zip.display().to_string(),
                    folder: folder.display().to_string(),
                }),
                Err(failure) => finish_download(state, Err(failure.into())),
            }
        }
        // The Windows setup and Linux AppImage are verified as downloaded.
        Next::Downloaded(file) => state.told.push_back(Told::Verify {
            app: file.display().to_string(),
        }),
        Next::Failed(failure) => finish_download(state, Err(failure)),
    }
}

fn finish_download(state: &mut State, result: Result<PathBuf, Words>) {
    state.updates.staged = result.as_ref().ok().cloned();
    state
        .updates
        .controller
        .downloaded_result(result.map(|_| ()));
}

/// The next shell effect (`Told`), or `None`. An unattended run prints each to stderr.
pub fn update_next_effect() -> Option<Told> {
    let language = crate::ffi::interface_language();
    let told = with(None, |state| loop {
        if let Some(told) = state.told.pop_front() {
            break Some(told);
        }
        let effect = state.updates.next_effect()?;
        if let Some(told) = take(state, effect, language) {
            break Some(told);
        }
    });
    if let (Some(said), true) = (&told, crate::launch::get().unattended) {
        let json = serde_json::to_string(said).expect("an effect is plain data");
        eprintln!("update\t{json}");
    }
    told
}

/// The updater's next request for the transport.
pub(crate) fn next_request() -> Option<(Step, Request)> {
    with(None, |state| state.outgoing.pop_front())
}

/// Delivers the reply to an updater request.
pub(crate) fn answered(step: Step, reply: &Reply) {
    with((), |state| match step {
        Step::Fetch => state
            .updates
            .fetched(reply.status, &reply.body, &reply.failure),
        Step::Download { url, zip, partial } => {
            let next = state.updates.downloaded(&url, &zip, &partial, reply);
            follow(state, next);
        }
    });
}

/// Reports `ditto`'s exit status, or why it could not run (`failure`). Then finds the app with
/// this app's identifier in the folder and requests its signature check.
///
/// # Safety
/// `folder` and `failure` are null or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_update_unpacked(
    folder: *const c_char,
    exit: i32,
    failure: *const c_char,
) {
    // SAFETY: as the caller promises.
    let (folder, failure) = unsafe { (text_of(folder), text_of(failure)) };
    with((), |state| {
        let running = &state.running;
        let bundle_name = running
            .bundle
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let identifier_of = |app: &Path| -> Option<String> {
            let path = CString::new(app.to_string_lossy().as_bytes()).ok()?;
            let mut buffer = vec![0u8; 1024];
            let written = (running.identifier_of)(path.as_ptr(), buffer.as_mut_ptr(), buffer.len())
                .min(buffer.len());
            (written > 0).then(|| String::from_utf8_lossy(&buffer[..written]).into_owned())
        };
        match super::unpacked(
            Path::new(&folder),
            exit,
            &failure,
            &bundle_name,
            &running.identifier,
            identifier_of,
        ) {
            Ok(app) => state.told.push_back(Told::Verify {
                app: app.display().to_string(),
            }),
            Err(failure) => finish_download(state, Err(failure)),
        }
    });
}

/// Reports the signature check of the unpacked app or downloaded file: `check` is the result,
/// with the system's `reason` or else a `status` error code on failure. On success it is staged
/// for install; otherwise the folder or file is removed and the download fails with the reason.
///
/// # Safety
/// `app` and `reason` are null or NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_update_verified(
    app: *const c_char,
    check: NrSignature,
    status: i32,
    reason: *const c_char,
) {
    // SAFETY: as the caller promises.
    let (app, reason) = unsafe { (PathBuf::from(text_of(app)), text_of(reason)) };
    with((), |state| {
        let Some(refusal) = refusal(check, status, &reason, &app) else {
            return finish_download(state, Ok(app));
        };
        if app.is_dir() {
            if let Some(folder) = app.parent() {
                let _ = std::fs::remove_dir_all(folder);
            }
        } else {
            let _ = std::fs::remove_file(&app);
        }
        finish_download(state, Err(refusal.into()));
    });
}

/// Checks the Linux AppImage's minisign signature from the channel file (`signature`) against
/// a local check's key or the built-in key, and for the offered version. Returns the result in
/// the same shape as a platform check.
pub fn checked_signature(file: &Path) -> (NrSignature, i32, String) {
    use super::signature::{check_file, Refusal, BUILT_IN_KEY};
    let local = with_updates(false, |updates| updates.feed_is_local());
    let named = variable("NR_UPDATE_REQUIREMENT").filter(|_| local);
    let key = named.as_deref().unwrap_or(BUILT_IN_KEY);
    let chosen = with_updates(None, |updates| updates.chosen.clone());
    let Some(chosen) = chosen else {
        return (
            NrSignature::Unreadable,
            0,
            "no update was chosen".to_owned(),
        );
    };
    match check_file(file, chosen.signature.as_deref(), key, &chosen.version) {
        Ok(()) => (NrSignature::Passed, 0, String::new()),
        Err(Refusal::NoKey) => (NrSignature::OwnUnreadable, 0, String::new()),
        Err(Refusal::Unreadable(reason)) => (NrSignature::Unreadable, 0, reason),
        Err(Refusal::Failed(reason)) => (NrSignature::Failed, 0, reason),
    }
}

/// The chosen update's version, which its signed file must match (the shell checks the Windows
/// setup's); empty before one is chosen.
pub fn chosen_version() -> String {
    with_updates(String::new(), |updates| {
        updates
            .chosen
            .as_ref()
            .map_or_else(String::new, |chosen| chosen.version.clone())
    })
}

/// Called on quit. Returns the waiting update to install, else empty; report the result with
/// `nr_update_installed`.
#[no_mangle]
pub extern "C" fn nr_update_install_at_quit() -> NrText {
    let app = with(None, |state| {
        let app = state
            .updates
            .staged
            .clone()
            .filter(|_| state.updates.controller.downloaded())?;
        state.installing = Some((Installing::Quit, app.clone()));
        Some(app)
    });
    lend(app.map(|app| app.display().to_string()).unwrap_or_default())
}

/// Reports the install result: `check` as in `nr_update_verified` for the re-check, or
/// `NotReplaced` when it passed but replacing failed (`reason` from the system).
///
/// # Safety
/// `reason` is null or a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_update_installed(
    check: NrSignature,
    status: i32,
    reason: *const c_char,
) {
    use crate::diagnostics::{write, write_with, Level};
    // SAFETY: as the caller promises.
    let reason = unsafe { text_of(reason) };
    with((), |state| {
        let Some((installing, app)) = state.installing.take() else {
            return;
        };
        if let Some(refusal) = refusal(check, status, &reason, &app) {
            return match installing {
                Installing::Relaunch => state.updates.controller.install_failed(refusal.into()),
                Installing::Quit => write(
                    Level::Error,
                    &format!("[updater] the update did not install at quit: {refusal}"),
                ),
            };
        }
        state.updates.staged = None;
        let version = state
            .updates
            .chosen
            .as_ref()
            .map(|chosen| chosen.version.clone())
            .unwrap_or_default();
        let at = state.running.bundle.display().to_string();
        write_with(
            Level::Info,
            "[updater] installed",
            &serde_json::json!({ "version": version, "at": at }),
        );
        if crate::launch::get().unattended {
            eprintln!("update\tinstalled\t{version}\t{at}");
        }
        if installing == Installing::Relaunch {
            state.told.push_back(Told::Relaunch);
        }
    });
}

/// The relaunched app's environment: this one's (so a check keeps its scratch folders), minus
/// dialog answers, plus the marker that a check's new app was launched.
pub fn update_relaunch_environment() -> BTreeMap<String, String> {
    relaunch_environment(std::env::vars_os(), crate::launch::get().unattended)
}

fn relaunch_environment(
    variables: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    unattended: bool,
) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> = variables
        .map(|(name, value)| {
            (
                name.to_string_lossy().into_owned(),
                value.to_string_lossy().into_owned(),
            )
        })
        .filter(|(name, _)| name != "NR_UPDATE_ANSWERS")
        .collect();
    if unattended {
        environment.insert("NR_DO".to_owned(), "update:launched".into());
    }
    environment
}

/// Delay before an unattended run answers a dialog, so its screenshot is taken first.
const ANSWER_AFTER_SECONDS: f64 = 0.3;

/// An unattended answer to a dialog: the button (-1 for none, so the dialog stays) and the
/// delay in seconds before pressing it.
#[repr(C)]
pub struct NrBoxAnswer {
    pub button: i32,
    pub after: f64,
}

/// The next `NR_UPDATE_ANSWERS` entry for a dialog with `buttons` buttons, or -1 (the dialog
/// stays) when it is out of range or none are left.
#[no_mangle]
pub extern "C" fn nr_update_take_answer(buttons: u32) -> NrBoxAnswer {
    static ANSWERS: OnceLock<Mutex<VecDeque<i64>>> = OnceLock::new();
    let answers = ANSWERS.get_or_init(|| {
        let text = variable("NR_UPDATE_ANSWERS").unwrap_or_default();
        Mutex::new(
            text.split(',')
                .filter_map(|answer| answer.parse().ok())
                .collect(),
        )
    });
    let answer = answers.relock().pop_front();
    let button = answer
        .filter(|answer| (0..i64::from(buttons)).contains(answer))
        .map_or(-1, |answer| answer as i32);
    NrBoxAnswer {
        button,
        after: ANSWER_AFTER_SECONDS,
    }
}

/// The next unattended dialog name: `box-1`, `box-2`… in display order.
pub fn update_box_name() -> String {
    static SHOWN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    format!(
        "box-{}",
        SHOWN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    )
}

/// Reports the index of the button pressed in the shown dialog.
#[no_mangle]
pub extern "C" fn nr_update_answered(button: u32) {
    with_updates((), |updates| updates.controller.answered(button as usize));
}

/// The Northroom menu item's label for the status, in the interface language.
pub(crate) fn menu_label() -> &'static str {
    let language = crate::ffi::interface_language();
    strings::text(
        language,
        with_updates(Status::Idle.menu_label(), |updates| {
            updates.controller.status().menu_label()
        }),
    )
}

pub(crate) fn menu_enabled() -> bool {
    with_updates(true, |updates| updates.controller.status().menu_enabled())
}

/// The status the title bar button shows: the updater's, or a screenshot override.
fn button_status() -> Status {
    let pictured = *PICTURED.relock();
    pictured.unwrap_or_else(|| with_updates(Status::Idle, |updates| updates.controller.status()))
}

/// The title bar update button's label; empty while hidden.
#[no_mangle]
pub extern "C" fn nr_update_button_label() -> NrText {
    let language = crate::ffi::interface_language();
    lend(
        button_status()
            .button_label()
            .map_or("", |label| strings::text(language, label))
            .to_owned(),
    )
}

/// Whether the title bar button shows a download in progress (its spinner).
#[no_mangle]
pub extern "C" fn nr_update_button_downloading() -> bool {
    button_status() == Status::Downloading
}

/// Overrides the title bar button status with `name` (`idle`, `checking`, `available`,
/// `downloading`, `ready`) for screenshots; any other name clears the override.
pub(crate) fn picture(name: &str) {
    *PICTURED.relock() = Status::named(name);
}

/// Whether a downloaded update waits to install when the app quits.
pub(crate) fn update_ready() -> bool {
    with_updates(false, |updates| updates.controller.downloaded())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Copies the text an `NrText` lends.
    fn read(text: &NrText) -> String {
        // SAFETY: `length` bytes at `bytes`, alive until this thread's next lend.
        unsafe { text.to_owned_string() }
    }

    #[test]
    fn text_lent_on_one_thread_outlives_text_lent_on_another() {
        let here = lend("the main thread's".to_owned());
        std::thread::spawn(|| {
            let there = lend("another thread's, longer than the first".to_owned());
            assert_eq!(read(&there), "another thread's, longer than the first");
        })
        .join()
        .expect("the other thread");
        assert_eq!(read(&here), "the main thread's");
    }

    #[test]
    fn a_refused_signature_is_said_with_its_path_and_the_systems_reason() {
        let app = Path::new("/tmp/u/Northroom.app");
        assert_eq!(refusal(NrSignature::Passed, 0, "", app), None);
        assert_eq!(
            refusal(NrSignature::Unreadable, -67062, "", app).as_deref(),
            Some("Code signature at URL /tmp/u/Northroom.app could not be read.")
        );
        assert_eq!(
            refusal(NrSignature::OwnUnreadable, 0, "", app).as_deref(),
            Some("This app's own code signature could not be read.")
        );
        assert_eq!(
            refusal(
                NrSignature::Failed,
                -67030,
                "a sealed resource is missing or invalid",
                app
            )
            .as_deref(),
            Some(
                "Code signature at URL /tmp/u/Northroom.app did not pass validation: a sealed resource is missing or invalid"
            )
        );
        assert_eq!(
            refusal(NrSignature::Failed, -67050, "", app).as_deref(),
            Some(
                "Code signature at URL /tmp/u/Northroom.app did not pass validation: OSStatus -67050"
            ),
            "the error code when the system has no words"
        );
        assert_eq!(
            refusal(
                NrSignature::NotReplaced,
                0,
                "You don’t have permission.",
                app
            )
            .as_deref(),
            Some("You don’t have permission.")
        );
    }

    #[test]
    fn an_effect_crosses_as_json_named_by_its_kind() {
        let shown = Told::Show(UpdateBox {
            error: true,
            message: "Could not complete the update".into(),
            detail: "tab\there".into(),
            buttons: vec!["OK"],
            return_button: None,
            escape_button: Some(0),
        });
        assert_eq!(
            serde_json::to_value(&shown).expect("plain data"),
            serde_json::json!({"kind": "show", "error": true, "message": "Could not complete the update", "detail": "tab\there",
                "buttons": ["OK"], "return_button": null, "escape_button": 0})
        );
        assert_eq!(
            serde_json::to_string(&Told::Relaunch).expect("plain data"),
            r#"{"kind":"relaunch"}"#
        );
    }

    #[test]
    fn the_new_app_opens_with_this_ones_environment_less_its_answers() {
        let variables = || {
            [
                ("NR_WORKSPACE", "/tmp/w"),
                ("NR_UPDATE_ANSWERS", "0,0"),
                ("NR_DO", "update:check"),
            ]
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
        };
        let person = relaunch_environment(variables(), false);
        assert_eq!(
            person.get("NR_WORKSPACE").map(String::as_str),
            Some("/tmp/w")
        );
        assert_eq!(
            person.get("NR_DO").map(String::as_str),
            Some("update:check")
        );
        assert!(!person.contains_key("NR_UPDATE_ANSWERS"));
        let check = relaunch_environment(variables(), true);
        assert_eq!(
            check.get("NR_DO").map(String::as_str),
            Some("update:launched"),
            "a check's new app says it was launched"
        );
    }
}
