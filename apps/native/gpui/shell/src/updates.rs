//! App updates (`core/src/update`). The core decides the feed, whether an update exists, what to
//! download, and what to tell the user when. This module does the platform work: unpacking the
//! zip with `ditto`, reading a bundle identifier, checking the code signature
//! (`kit::platform::signature`), showing alerts, and swapping in and launching the new app. The
//! title bar button and menu item follow the updater's status. In a check, alerts are printed to
//! stderr and answered as the core scripts.
//!
//! On Windows the update is the per-user setup. Its Authenticode signer key must match the running
//! app's and its version the offered one (`kit::platform::signature`); installing runs it silently
//! over the install folder after this app quits, relaunching if asked. On Linux it is the
//! AppImage: the core checks its minisign signature and replaces the running one (`$APPIMAGE`),
//! which relaunches as on a Mac. A build run from its folder (no `Resources/release.json`) never
//! updates itself.

use crate::workspace::Workspace;
use gpui::{App, AsyncApp, WindowHandle};
use kit::core::{c, text};
use kit::nr::*;
use northroom_core::update::ffi::{Told, UpdateBox};
use std::time::Duration;

/// The window that shows update status and alerts.
fn window(cx: &App) -> Option<WindowHandle<Workspace>> {
    cx.windows()
        .into_iter()
        .find_map(|window| window.downcast::<Workspace>())
}

/// The running app's version, the bundle or package an update replaces, and its identifier.
/// All empty for a build run from its folder.
pub fn running() -> (String, String, String) {
    #[cfg(target_os = "macos")]
    return kit::platform::running_bundle();
    #[cfg(not(target_os = "macos"))]
    return northroom_core::update::package::running();
}

/// Starts the updater and, in an installed release, its schedule: a check 30 s after launch,
/// then every 6 hours.
pub fn start(cx: &mut App) {
    let (version, bundle, identifier) = running();
    let (version, bundle, identifier) = (c(&version), c(&bundle), c(&identifier));
    // SAFETY: NUL-terminated strings.
    unsafe {
        nr_update_open(
            version.as_ptr(),
            bundle.as_ptr(),
            identifier.as_ptr(),
            identifier_of,
        )
    };
    if !nr_update_supported() {
        return;
    }
    cx.spawn(async move |cx| {
        let mut wait = nr_update_schedule(true);
        loop {
            cx.background_executor()
                .timer(Duration::from_secs_f64(wait))
                .await;
            cx.update(|cx| request(false, cx));
            wait = nr_update_schedule(false);
        }
    })
    .detach();
}

#[cfg(target_os = "macos")]
extern "C" fn identifier_of(path: *const std::ffi::c_char, out: *mut u8, capacity: usize) -> usize {
    kit::platform::bundle_identifier(path, out, capacity)
}

/// Only Mac bundles have an identifier.
#[cfg(not(target_os = "macos"))]
extern "C" fn identifier_of(_: *const std::ffi::c_char, _: *mut u8, _: usize) -> usize {
    0
}

/// Requests an update check. `manual` is true from the menu, tray or title bar button, false
/// from the schedule.
pub fn request(manual: bool, cx: &mut App) {
    nr_update_request(manual);
    // Defer until the calling window's update ends, since the status redraws the window and an
    // alert attaches to it.
    cx.defer(pump);
}

/// Performs the core's update tasks in order until it waits on something, then sends its queued
/// requests.
pub fn pump(cx: &mut App) {
    while let Some(effect) = update_next_effect() {
        match effect {
            Told::Status { .. } => {
                if let Some(window) = window(cx) {
                    let _ =
                        window.update(cx, |workspace, window, cx| workspace.changed(window, cx));
                }
            }
            Told::Show(shown) => show(shown, cx),
            Told::Unpack { zip, folder } => unpack(zip, folder, cx),
            Told::Verify { app } => verify(app, cx),
            Told::Install { app } => {
                let (check, status, reason) = install(&app, Then::Relaunch);
                let reason = c(&reason);
                // SAFETY: a NUL-terminated reason.
                unsafe { nr_update_installed(check, status, reason.as_ptr()) };
            }
            Told::Relaunch => relaunch(cx),
        }
    }
    crate::news::pump_transport();
}

// MARK: Unpacking and checking

/// Unpacks the zip with `ditto`, as Squirrel.Mac does, off the UI thread.
fn unpack(zip: String, folder: String, cx: &mut App) {
    let unpacked = cx.background_executor().spawn({
        let folder = folder.clone();
        async move {
            match std::process::Command::new("/usr/bin/ditto")
                .args(["-x", "-k", &zip, &folder])
                .status()
            {
                Ok(status) => (status.code().unwrap_or(-1), String::new()),
                Err(error) => (0, error.to_string()),
            }
        }
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let (exit, failure) = unpacked.await;
        let (folder, failure) = (c(&folder), c(&failure));
        // SAFETY: NUL-terminated strings.
        unsafe { nr_update_unpacked(folder.as_ptr(), exit, failure.as_ptr()) };
        cx.update(pump);
    })
    .detach();
}

/// Checks the unpacked app's signature, off the UI thread.
fn verify(app: String, cx: &mut App) {
    let checked = cx.background_executor().spawn({
        let app = app.clone();
        async move { signature(&app) }
    });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let (check, status, reason) = checked.await;
        let (app, reason) = (c(&app), c(&reason));
        // SAFETY: NUL-terminated strings.
        unsafe { nr_update_verified(app.as_ptr(), check, status, reason.as_ptr()) };
        cx.update(pump);
    })
    .detach();
}

/// Checks the signature of the app at `app` against the requirement a check sets, or else the
/// running app's.
#[cfg(target_os = "macos")]
fn signature(app: &str) -> (NrSignature, i32, String) {
    kit::platform::signature(app, &text(&nr_update_requirement()))
}

/// Checks the setup's Authenticode signer key against the one a check sets (or else the running
/// app's) and its version against the offered one.
#[cfg(target_os = "windows")]
fn signature(setup: &str) -> (NrSignature, i32, String) {
    kit::platform::signature(
        setup,
        &text(&nr_update_requirement()),
        &northroom_core::update::ffi::chosen_version(),
    )
}

/// Checks the AppImage's minisign signature in the core.
#[cfg(target_os = "linux")]
fn signature(appimage: &str) -> (NrSignature, i32, String) {
    northroom_core::update::ffi::checked_signature(std::path::Path::new(appimage))
}

// MARK: Installing

/// What an update replaces: the app bundle (Mac), install folder (Windows) or AppImage (Linux).
fn bundle() -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    let (_, bundle, _) = kit::platform::running_bundle();
    #[cfg(not(target_os = "macos"))]
    let (_, bundle, _) = northroom_core::update::package::running();
    std::path::PathBuf::from(bundle)
}

/// What follows an install: relaunch (Restart and Install), or quit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Then {
    Relaunch,
    Quit,
}

/// Re-checks the signature, then swaps the update at `update` in for this app. Returns the
/// signature result, or `NotReplaced` with the system's error.
#[cfg(target_os = "macos")]
fn install(update: &str, _: Then) -> (NrSignature, i32, String) {
    let found = signature(update);
    if found.0 != NrSignature::Passed {
        return found;
    }
    let running = bundle();
    let aside = running.with_extension("app.replaced");
    let replaced = std::fs::rename(&running, &aside)
        .and_then(|()| {
            std::fs::rename(update, &running).inspect_err(|_| {
                let _ = std::fs::rename(&aside, &running);
            })
        })
        .and_then(|()| std::fs::remove_dir_all(&aside));
    match replaced {
        Ok(()) => found,
        Err(error) => (NrSignature::NotReplaced, 0, error.to_string()),
    }
}

/// Starts the checked setup silently over the install folder (Inno Setup per-user install,
/// `scripts/windows/northroom.iss`). It waits for this app to exit, installs, and relaunches with
/// the core's environment when asked. A failed start returns the system's error.
#[cfg(target_os = "windows")]
fn install(update: &str, then: Then) -> (NrSignature, i32, String) {
    let found = signature(update);
    if found.0 != NrSignature::Passed {
        return found;
    }
    let mut setup = std::process::Command::new(update);
    setup.args([
        "/VERYSILENT".to_owned(),
        "/SUPPRESSMSGBOXES".to_owned(),
        "/NORESTART".to_owned(),
        "/CURRENTUSER".to_owned(),
        format!("/DIR={}", bundle().display()),
        format!("/WAITPID={}", std::process::id()),
    ]);
    if then == Then::Relaunch {
        setup
            .arg("/RELAUNCH=1")
            .env_clear()
            .envs(update_relaunch_environment());
    }
    match setup.spawn() {
        Ok(_) => found,
        Err(error) => (NrSignature::NotReplaced, 0, error.to_string()),
    }
}

/// Replaces the running AppImage with the checked one (the core's `put_in_place`).
#[cfg(target_os = "linux")]
fn install(update: &str, _: Then) -> (NrSignature, i32, String) {
    let found = signature(update);
    if found.0 != NrSignature::Passed {
        return found;
    }
    match northroom_core::update::put_in_place(std::path::Path::new(update), &bundle()) {
        Ok(()) => found,
        Err(reason) => (NrSignature::NotReplaced, 0, reason),
    }
}

/// Restart and Install: a small shell waiter outlives this process and opens the new app with the
/// core's environment once this one exits.
#[cfg(not(target_os = "windows"))]
fn relaunch(cx: &mut App) {
    let environment = update_relaunch_environment();
    // On a Mac launch the executable inside the bundle; an AppImage is itself the executable.
    #[cfg(target_os = "macos")]
    let program = std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_owned()))
        .map(|name| bundle().join("Contents/MacOS").join(name));
    #[cfg(not(target_os = "macos"))]
    let program = Some(bundle());
    let started = program.map(|program| {
        std::process::Command::new("/bin/sh")
            .args([
                "-c",
                "while kill -0 \"$1\" 2>/dev/null; do sleep 0.1; done; exec \"$2\"",
                "sh",
                &std::process::id().to_string(),
                &program.display().to_string(),
            ])
            .env_clear()
            .envs(environment)
            .spawn()
    });
    if !matches!(started, Some(Ok(_))) {
        eprintln!("[updater] the new version could not be opened");
    }
    cx.quit();
}

/// Restart and Install on Windows. The setup started by `install` waits for this app to quit,
/// then installs and opens the new app.
#[cfg(target_os = "windows")]
fn relaunch(cx: &mut App) {
    cx.quit();
}

/// Installs an update deferred with Later as the app quits.
pub fn install_on_quit() {
    let update = text(&nr_update_install_at_quit());
    if update.is_empty() {
        return;
    }
    let (check, status, reason) = install(&update, Then::Quit);
    let reason = c(&reason);
    // SAFETY: a NUL-terminated reason.
    unsafe { nr_update_installed(check, status, reason.as_ptr()) };
}

// MARK: Alerts

/// Shows an update alert (an `NSAlert` on a Mac) with Return and Escape on the buttons the core
/// names. A check prints it and answers as the core scripts.
fn show(shown: UpdateBox, cx: &mut App) {
    if nr_launch().unattended {
        describe(&shown);
        let taken = nr_update_take_answer(shown.buttons.len() as u32);
        let Ok(answer) = usize::try_from(taken.button) else {
            return;
        };
        let title = shown.buttons[answer];
        cx.spawn(async move |cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_secs_f64(taken.after))
                .await;
            eprintln!("update\tanswer\t{answer}\t{title}");
            nr_update_answered(answer as u32);
            cx.update(pump);
        })
        .detach();
        return;
    }
    let style = if shown.error {
        BoxStyle::Critical
    } else {
        BoxStyle::Informational
    };
    let held = HeldBox {
        style,
        message: shown.message,
        detail: shown.detail,
        buttons: shown
            .buttons
            .iter()
            .map(|&button| button.to_owned())
            .collect(),
        return_button: shown.return_button,
        escape_button: shown.escape_button,
    };
    present(held, cx, |answer, cx| {
        nr_update_answered(answer as u32);
        pump(cx);
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxStyle {
    Informational,
    Critical,
}

/// An alert for the user. Return presses `return_button`, or the first button when unset. No
/// buttons means the platform's OK.
pub struct HeldBox {
    pub style: BoxStyle,
    pub message: String,
    pub detail: String,
    pub buttons: Vec<String>,
    pub return_button: Option<usize>,
    pub escape_button: Option<usize>,
}

/// Shows `held` and calls `answered` with the pressed button's index. On a Mac this is an app
/// `NSAlert`, because GPUI's prompt is a sheet and always gives Return to the first button.
/// Elsewhere it is the window's drawn alert.
pub fn present(held: HeldBox, cx: &mut App, answered: impl FnOnce(usize, &mut App) + 'static) {
    #[cfg(target_os = "macos")]
    cx.spawn(async move |cx: &mut AsyncApp| {
        // The alert runs a modal loop, so it must run outside any app update.
        let style = match held.style {
            BoxStyle::Informational => kit::platform::AlertStyle::Informational,
            BoxStyle::Critical => kit::platform::AlertStyle::Critical,
        };
        let answer = kit::platform::run_alert(&kit::platform::Alert {
            style,
            message: &held.message,
            detail: &held.detail,
            buttons: &held.buttons,
            return_button: held.return_button,
            escape_button: held.escape_button,
        });
        cx.update(|cx| answered(answer, cx));
    })
    .detach();
    #[cfg(not(target_os = "macos"))]
    {
        let Some(handle) = window(cx) else {
            return;
        };
        let answered = std::cell::Cell::new(Some(answered));
        let answered: kit::dialog::Answered = std::rc::Rc::new(move |answer, _, cx| {
            if let Some(answered) = answered.take() {
                answered(answer, cx);
            }
        });
        let shown = kit::dialog::AlertBox {
            message: held.message,
            detail: held.detail,
            buttons: held.buttons,
            return_button: held.return_button,
            escape_button: held.escape_button,
            critical: held.style == BoxStyle::Critical,
        };
        let _ = handle.update(cx, |workspace, window, cx| {
            workspace.overlay.dialogs.update(cx, |dialogs, cx| {
                dialogs.show_alert(shown, answered, window, cx)
            })
        });
    }
}

/// Prints an alert as one line, `dialog<TAB>name<TAB>style<TAB>message<TAB>detail<TAB>buttons`,
/// with each button's key.
fn describe(shown: &UpdateBox) {
    let name = update_box_name();
    let keys: Vec<String> = shown
        .buttons
        .iter()
        .enumerate()
        .map(|(index, title)| {
            let key = if Some(index) == shown.return_button.or(Some(0)) {
                "return"
            } else if Some(index) == shown.escape_button {
                "escape"
            } else {
                "-"
            };
            format!("{title}={key}")
        })
        .collect();
    let style = if shown.error {
        "critical"
    } else {
        "informational"
    };
    eprintln!(
        "dialog\t{}\t{style}\t{}\t{}\t{}",
        name,
        shown.message,
        shown.detail,
        keys.join("|")
    );
}
