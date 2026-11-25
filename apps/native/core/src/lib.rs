//! Northroom's native core (ADR-0001). A platform shell links this one library: it sends
//! commands, reads the view state the core publishes, and hands over native GPU layers that
//! the core draws into on its own timing. No shell types appear here.

pub mod app_ffi;
pub mod archive;
pub mod board;
mod calendar;
pub mod changes;
pub mod colour_picker;
pub mod columns;
pub mod commands;
pub mod deck;
pub mod diagnostics;
pub mod dialog;
mod digest;
pub mod drive;
pub mod ffi;
pub mod geometry;
pub mod glyph;
pub mod highlight;
pub mod launch;
pub mod list;
mod media;
pub mod menus;
pub mod model_still;
pub mod motion;
pub mod package;
pub mod page;
pub mod paths;
pub mod preview;
pub mod script;
pub mod scroll;
pub mod sessions;
pub mod settings;
pub mod sheets;
pub mod strings;
pub mod terminal;
pub mod text;
pub mod theme;
mod threads;
pub mod transport;
pub mod update;
pub mod window;
pub mod workspace;

use northroom_media::gpu::Gpu;
use northroom_media::{MediaError, MediaResult};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard};

/// Takes a lock even if a thread panicked while holding it. No state behind a core lock is
/// worth failing every later caller over, so the app carries on with what is there.
pub trait Relock<T: ?Sized> {
    fn relock(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> Relock<T> for Mutex<T> {
    fn relock(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// All state the core owns for the running app.
pub(crate) struct App {
    pub preview: Mutex<Option<preview::Preview>>,
    /// Running sessions by terminal file id. A session keeps running while another file is
    /// open and comes back as it was left.
    pub sessions: Mutex<std::collections::HashMap<String, terminal::Terminal>>,
    /// The terminals on screen, and which one has keyboard focus.
    pub shown_terminals: Mutex<crate::terminal::screen::Shown>,
    /// Each session's state as of the last "waiting" notification.
    pub session_watch: Mutex<sessions::Watch>,
    /// The drive's columns, once a workspace folder is open.
    pub columns: Mutex<Option<columns::Columns>>,
    /// Watches the folders the open columns list.
    pub watcher: Mutex<Option<drive::watch::FolderWatch>>,
    pub theme: Mutex<theme::Theme>,
    /// Platform callbacks for file index, kind and trash (`nr_drive_start`), kept so a newly
    /// chosen drive gets them too.
    pub files: Mutex<ffi::NrFileSystem>,
    /// Shell callback for changes made off its thread.
    pub changed: Mutex<Option<extern "C" fn()>>,
    /// The app support folder and the open drive folder.
    pub support: Mutex<Option<(std::path::PathBuf, std::path::PathBuf)>>,
    /// The settings tab on screen, if any, and a revision counter.
    pub settings: Mutex<(Option<u32>, u64)>,
    /// Appearance settings, and whether the system is in dark mode.
    pub appearance: Mutex<(settings::appearance::Settings, bool)>,
    /// The full-window page, once the shell asks for it.
    pub page: Mutex<Option<page::Page>>,
    /// The open page's players, and which one is active.
    pub page_players: Mutex<crate::page::players::PagePlayers>,
    pub page_history: Mutex<crate::page::edit::History>,
    /// The list rows last requested, kept alive so the shell can borrow their text.
    pub list_rows: Mutex<Vec<columns::ListRow>>,
    /// The board cards last requested, kept alive for the same reason.
    pub board_rows: Mutex<Vec<workspace::Entry>>,
    /// The frames of those cards from the last layout.
    pub board_frames: Mutex<Vec<board::Frame>>,
    /// The open page's file id, and the text last read or written.
    pub page_file: Mutex<Option<(String, String)>>,
    /// What terminals changed in their folders (`changes`), started on first use.
    pub changes: OnceLock<changes::service::Service>,
}

static APP: OnceLock<App> = OnceLock::new();
static GPU: OnceLock<Result<Arc<Gpu>, String>> = OnceLock::new();

/// wgpu won't reconfigure a surface while another thread submits to the same device (it reports
/// the GPU never went idle). Drawing threads hold `drawing()` while encoding and submitting a
/// frame and `reconfiguring()` while configuring a surface, so frames run in parallel and a
/// reconfigure gets the device to itself.
static GPU_GATE: RwLock<()> = RwLock::new(());

pub(crate) fn drawing() -> RwLockReadGuard<'static, ()> {
    GPU_GATE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// Only macOS configures surfaces; elsewhere the core renders to images the shell displays.
#[cfg(target_os = "macos")]
pub(crate) fn reconfiguring() -> std::sync::RwLockWriteGuard<'static, ()> {
    GPU_GATE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The single GPU device all core drawing shares (ADR-0001).
pub(crate) fn gpu() -> MediaResult<Arc<Gpu>> {
    GPU.get_or_init(|| Gpu::new().map(Arc::new).map_err(|error| error.to_string()))
        .clone()
        .map_err(MediaError)
}
/// Tells the shell that something it draws changed off its thread, such as terminal activity.
/// The shell redraws when the revision it reads next differs.
pub(crate) fn notify_changed() {
    let Some(app) = APP.get() else {
        return;
    };
    app.settings.relock().1 += 1;
    let changed = *app.changed.relock();
    if let Some(changed) = changed {
        changed();
    }
}

pub(crate) fn app() -> &'static App {
    APP.get().expect("nr_app_start has not been called")
}

pub(crate) fn start() {
    let _ = APP.set(App {
        preview: Mutex::new(None),
        sessions: Mutex::new(std::collections::HashMap::new()),
        session_watch: Mutex::new(sessions::Watch::default()),
        shown_terminals: Mutex::new(crate::terminal::screen::Shown::default()),
        columns: Mutex::new(None),
        page: Mutex::new(None),
        page_players: Mutex::new(crate::page::players::PagePlayers::default()),
        page_history: Mutex::new(crate::page::edit::History::default()),
        list_rows: Mutex::new(Vec::new()),
        board_rows: Mutex::new(Vec::new()),
        board_frames: Mutex::new(Vec::new()),
        page_file: Mutex::new(None),
        changes: OnceLock::new(),
        watcher: Mutex::new(None),
        files: Mutex::new(ffi::NrFileSystem::NONE),
        changed: Mutex::new(None),
        support: Mutex::new(None),
        settings: Mutex::new((None, 0)),
        appearance: Mutex::new((settings::appearance::Settings::default(), true)),
        theme: Mutex::new(
            theme::derive(
                &theme::Appearance::shipped(theme::Mode::Dark),
                theme::Mode::Dark,
            )
            .expect("the shipped appearance is valid"),
        ),
    });
}
