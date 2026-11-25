//! Northroom's GPUI shell (ADR-0001). It holds no product logic: it starts the core, opens the
//! window, draws what the core publishes with kit's components, forwards pointer and key input,
//! and runs a check's script.
//!
//! Layout: `workspace` is the window content, `screens` has one module per screen, `overlay`
//! holds menus, dialog, tooltip, notices and palette, `keys` handles keys no view takes,
//! `effects` does the platform side of core effects, `news` carries wake-ups from other threads,
//! and `script` runs check steps through handlers the window and screens register.

mod effects;
mod frame_times;
mod keys;
mod menu_bar;
mod model_stills;
mod news;
mod overlay;
mod screens;
mod script;
mod sessions;
mod transport;
mod updates;
mod workspace;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, AppContext, Bounds, TitlebarOptions, WindowAppearance, WindowBounds, WindowHandle,
    WindowOptions, point, px, size,
};
use kit::core::{c, text};
use kit::nr::*;
use kit::platform;

static INTER: &[u8] = include_bytes!("../../../core/assets/fonts/Inter-latin.woff2");
static GEIST_MONO: &[u8] = include_bytes!("../../../core/assets/fonts/GeistMono-latin.woff2");

fn main() {
    // SAFETY: runs first, while the process has one thread.
    #[cfg(target_os = "linux")]
    unsafe {
        platform::draw_glyphs_unsmoothed()
    };
    // Start the steady clock now so time to first window is measured from launch.
    kit::motion::now();
    #[cfg(target_os = "macos")]
    platform::draw_glyphs_unsmoothed();
    nr_app_start();
    open_app_folders();
    start_log();
    let attendance = Attendance::of(&nr_launch());
    // A launch path (`northroom <path>`) opens once the drive is ready.
    if let Some(path) = std::env::args_os().nth(1) {
        open_path(&path.to_string_lossy());
    }
    let app = gpui_platform::application();
    // Paths other programs ask us to open (`open -a Northroom <path>` on a Mac).
    app.on_open_urls(|urls| {
        for url in urls {
            if let Some(path) = file_path(&url) {
                open_path(&path);
            }
        }
    });
    app.run(move |cx: &mut App| start(attendance, cx));
}

/// Opens `path` in the drive (`nr_open_path`), printing any refusal to stderr.
fn open_path(path: &str) {
    let named = c(path);
    // SAFETY: a NUL-terminated path.
    let refusal = text(&unsafe { nr_open_path(named.as_ptr()) });
    if !refusal.is_empty() {
        eprintln!("{path} was not opened: {refusal}");
    }
}

/// Decodes the path of a `file://` URL. Returns `None` for any other URL.
fn file_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let escaped = (bytes[at] == b'%')
            .then(|| bytes.get(at + 1..at + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                at += 3;
            }
            None => {
                decoded.push(bytes[at]);
                at += 1;
            }
        }
    }
    let path = String::from_utf8(decoded).ok()?;
    // Windows paths arrive as `file:///C:/…`.
    Some(match path.as_bytes() {
        [b'/', _, b':', ..] => path[1..].to_owned(),
        _ => path,
    })
}

#[cfg(test)]
mod tests {
    use super::file_path;

    #[test]
    fn a_file_url_names_its_path_and_nothing_else_does() {
        assert_eq!(
            file_path("file:///Users/ada/My%20Work/a%C3%A9.html").as_deref(),
            Some("/Users/ada/My Work/aé.html")
        );
        assert_eq!(
            file_path("file://localhost/tmp/x").as_deref(),
            Some("/tmp/x")
        );
        assert_eq!(
            file_path("file:///C:/Work/a.txt").as_deref(),
            Some("C:/Work/a.txt")
        );
        assert_eq!(file_path("https://example.com/a"), None);
        assert_eq!(file_path("file:///100%").as_deref(), Some("/100%"));
    }
}

/// Whether a person or a check runs the app (`NrLaunch::unattended`). A check takes no focus,
/// stays out of the Dock and app switcher, and its window covers nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Attendance {
    Attended,
    Unattended,
}

impl Attendance {
    fn of(launch: &NrLaunch) -> Self {
        if launch.unattended {
            Self::Unattended
        } else {
            Self::Attended
        }
    }
}

/// Hands the core the platform's support, logs and caches folders.
fn open_app_folders() {
    let (support, logs, caches) = platform::app_folders();
    let (support, logs, caches) = (c(&support), c(&logs), c(&caches));
    // SAFETY: NUL-terminated paths.
    unsafe { nr_paths_open(support.as_ptr(), logs.as_ptr(), caches.as_ptr()) };
}

/// The app name written to the diagnostic log.
const APP_NAME: &str = "Northroom";

/// Opens the diagnostic log, starting with facts only the platform knows: app version, whether
/// it runs installed, and OS version.
fn start_log() {
    let (version, bundle, _) = updates::running();
    let (major, minor, patch) = platform::system_version();
    let system = format!("{major}.{minor}.{patch}");
    northroom_core::ffi::log_start(&northroom_core::diagnostics::Startup {
        app_name: APP_NAME,
        version: &version,
        packaged: installed(&bundle),
        system: &system,
    });
}

/// Whether the app runs from an installed bundle or package rather than a build folder. On a
/// Mac `updates::running` reports whatever folder holds the program, so check for `.app`.
fn installed(bundle: &str) -> bool {
    if cfg!(target_os = "macos") {
        bundle.ends_with(".app")
    } else {
        !bundle.is_empty()
    }
}

/// Starts the app inside GPUI: the drive, the window and everything that keeps it current.
fn start(attendance: Attendance, cx: &mut App) {
    #[cfg(target_os = "macos")]
    if attendance == Attendance::Unattended {
        platform::keep_out_of_the_dock();
    }
    install_text_and_keys(cx);
    open_appearance(cx);
    nr_set_fallback_measure(kit::text::measure_fallback);
    let woken = news::start();
    // Opens the check's drive or the user's chosen folder, with the platform's file metadata
    // (on a Mac, Finder tags and the file index; Finder kind names everywhere). A check whose
    // folder can't be opened quits; the core logs why.
    if nr_drive_start(platform::file_system()) == NrDriveStart::CheckFailed {
        cx.quit();
        return;
    }
    news::watch();
    model_stills::start(cx);
    northroom_core::deck::ffi::start_rendering(kit::wake::wake);
    let handle = open_window(attendance, cx);
    cx.on_window_closed(|cx, _| cx.quit()).detach();
    // A downloaded update the user deferred with Later installs at quit.
    menu_bar::install(handle, cx);
    sessions::install(handle, cx);
    updates::start(cx);
    // A paired phone can reconnect from now on (ADR-0016).
    northroom_core::remote::app::nr_phone_link_start();
    cx.on_app_quit(|_| {
        updates::install_on_quit();
        // Keep the open tabs for the next launch, then end every session, keeping its output,
        // and trash throwaway terminals, however the app quits.
        nr_tabs_keep();
        nr_terminals_quit();
        async {}
    })
    .detach();
    if attendance == Attendance::Attended {
        cx.activate(true);
    }
    follow_news(woken, handle, cx);
    script::run(handle, cx);
    frame_times::start(handle, cx);
    #[cfg(target_os = "linux")]
    pump_glib(cx);
}

/// Installs the app's fonts, text rendering and key bindings.
fn install_text_and_keys(cx: &mut App) {
    cx.text_system()
        .add_fonts(kit::fonts::faces(INTER, GEIST_MONO))
        .expect("the app's own faces load");
    screens::terminal::install_faces(cx);
    // Greyscale antialiasing, as on a Mac, never subpixel (ClearType).
    cx.set_text_rendering_mode(gpui::TextRenderingMode::Grayscale);
    kit::text::install(cx.text_system().clone());
    cx.bind_keys(kit::controls::input::bindings());
    cx.bind_keys(kit::lists::bindings());
    cx.bind_keys(crate::screens::page::bindings());
    cx.bind_keys(crate::screens::deck::bindings());
}

/// Tells the core the system appearance at launch.
fn open_appearance(cx: &mut App) {
    let dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    nr_appearance_start(dark);
}

fn open_window(attendance: Attendance, cx: &mut App) -> WindowHandle<workspace::Workspace> {
    let size_wanted = nr_window_size();
    let options = window_options(attendance, &size_wanted, cx);
    let handle = cx
        .open_window(options, |window, cx| {
            cx.new(|cx| workspace::Workspace::new(window, cx))
        })
        .expect("the window opens");
    // A check's window sits off screen, so it covers nothing and snapshots are the whole window
    // at 2 px per pt on any display. Frame-timing runs stay on screen to get frames.
    #[cfg(target_os = "macos")]
    if attendance == Attendance::Unattended && !frame_times::asked() {
        let _ = handle.update(cx, |_, window, _| {
            platform::put_check_window_off_screen(window, size_wanted.width, size_wanted.height);
        });
    }
    #[cfg(target_os = "windows")]
    if attendance == Attendance::Unattended {
        platform::draw_check_window_whole(handle.into(), size_wanted.width, size_wanted.height, cx);
    }
    // Closing the window quits, so it asks first while sessions are busy, as Quit does.
    let _ = handle.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, |window, cx| {
            if !nr_quit_asks() {
                return true;
            }
            let said = kit::lists::rename::Said::of(&nr_quit_or_ask());
            if let Some(Some(workspace)) = window.root::<workspace::Workspace>() {
                workspace.update(cx, |workspace, cx| {
                    effects::carry_out(workspace, said, window, cx);
                });
            }
            false
        });
    });
    // Report element focus to VoiceOver, not only window focus.
    #[cfg(target_os = "macos")]
    let _ = handle.update(cx, |_, window, _| {
        platform::report_accessibility_focus(window)
    });
    handle
}

/// A window centred at `size_wanted`, with the traffic lights where the core places them.
fn window_options(attendance: Attendance, size_wanted: &NrWindowSize, cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(
        None,
        size(px(size_wanted.width as f32), px(size_wanted.height as f32)),
        cx,
    );
    let buttons = nr_window_button_place(0);
    let unattended = attendance == Attendance::Unattended;
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: true,
            traffic_light_position: Some(point(px(buttons.x as f32), px(buttons.y as f32))),
        }),
        window_min_size: Some(size(
            px(size_wanted.min_width as f32),
            px(size_wanted.min_height as f32),
        )),
        focus: !unattended,
        // On a Mac a check's window opens hidden and is moved off screen
        // (`put_check_window_off_screen`), so it never shows over the user's windows.
        show: !(unattended && cfg!(target_os = "macos")),
        // GPUI throttles inactive windows to 30 fps. A check's window is never active but
        // should draw at full rate.
        inactive_frame_interval: None,
        // Draw in Display P3 on a Mac, as AppKit does, converting every colour first.
        use_display_p3: kit::display::DISPLAY_P3,
        // The app handles title bar presses so a tab can be dragged; the bar itself moves the
        // window (`TitleBar::mouse_down`). Otherwise AppKit would move the window when a drag
        // starts on a tab.
        app_owns_titlebar_drag: true,
        ..Default::default()
    }
}

/// Handles each window wake-up for the life of the app.
fn follow_news(
    mut woken: UnboundedReceiver<()>,
    handle: WindowHandle<workspace::Workspace>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        while woken.next().await.is_some() {
            // A loaded picture only needs a redraw; core news is taken in first.
            let news = news::take_news();
            let _ = handle.update(cx, |workspace, window, cx| {
                if news {
                    workspace.changed(window, cx);
                } else {
                    cx.notify();
                }
            });
            // Core news may include an answer to an updater request.
            if news {
                cx.update(updates::pump);
            }
        }
    })
    .detach();
}

/// WebKitGTK web views run on GLib's main context, which GPUI's loop doesn't drive. Pump it
/// every 4 ms while a web view is open, else every 50 ms.
#[cfg(target_os = "linux")]
fn pump_glib(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let wait = if webhost::run::pump() { 4 } else { 50 };
            cx.background_executor()
                .timer(std::time::Duration::from_millis(wait))
                .await;
        }
    })
    .detach();
}
