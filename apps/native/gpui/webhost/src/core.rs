//! Typed wrappers over the core's page host `nr_*` functions (`core/src/page/host/ffi.rs`). The
//! host for one web view answers the scheme, decides navigations, reads page messages and keeps
//! the log. Text the core lends is copied immediately, and its JSON is parsed into the types here.

use northroom_core::ffi::NrText;
use northroom_core::page::host::ffi as nr;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::path::PathBuf;
use std::sync::Arc;

/// Copies text the core lends into an owned string.
pub fn text(value: NrText) -> String {
    if value.bytes.is_null() || value.length == 0 {
        return String::new();
    }
    // SAFETY: the core lends `length` bytes of UTF-8 until its next call; we copy them here.
    String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(value.bytes, value.length) })
        .into_owned()
}

/// A C string for the core, with interior NULs stripped.
pub(crate) fn c(value: &str) -> CString {
    CString::new(value.replace('\0', "")).expect("no NUL is left")
}

/// The core's message for a page host problem (`NR_HOST_*`), with `detail` such as a platform
/// error, a folder or a codec.
pub fn problem(problem: u32, detail: &str) -> String {
    problem_at(problem, detail, 0, 0)
}

/// Like `problem`, with numbers the message names: a picture's `first` × `second`, or frame
/// `first`.
pub fn problem_at(problem: u32, detail: &str, first: u64, second: u64) -> String {
    let detail = c(detail);
    // SAFETY: a NUL-terminated string.
    text(unsafe { nr::nr_host_problem(problem, detail.as_ptr(), first, second) })
}

/// Why the page host, a bake or an export stopped.
#[derive(Debug, Clone, PartialEq)]
pub enum HostError {
    /// The web view, the encoder or the core failed, with the core's message.
    Host(String),
    /// A file could not be written.
    Unwritten(String),
    /// The user cancelled an export.
    Cancelled,
}

impl std::fmt::Display for HostError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Host(words) | Self::Unwritten(words) => formatter.write_str(words),
            Self::Cancelled => formatter.write_str(&problem(nr::NR_HOST_EXPORT_CANCELLED, "")),
        }
    }
}

impl std::error::Error for HostError {}

impl HostError {
    /// The core's message for `problem_code`.
    pub fn of(problem_code: u32, detail: &str) -> Self {
        Self::Host(problem(problem_code, detail))
    }
}

/// Parses the core's JSON into `T`, or returns the core's `unread` error with the text.
pub fn decoded<T: DeserializeOwned>(text: &str, unread: u32) -> Result<T, HostError> {
    serde_json::from_str(text).map_err(|_| HostError::of(unread, text))
}

/// Parses core state the shell cannot run without. The core serializes it from its own types,
/// so a parse failure is a bug and panics.
fn state<T: DeserializeOwned>(text: &str) -> T {
    match serde_json::from_str(text) {
        Ok(value) => value,
        Err(error) => panic!("{}: {error}", problem(nr::NR_HOST_CORE_UNREAD, text)),
    }
}

/// The folder of pinned libraries and fonts the host serves under `/__nr/` (`vendor/`, `fonts/`).
/// Packaged builds find it next to the binary: `Contents/Resources/SceneHost` in a Mac bundle
/// (from `Contents/MacOS` or `Contents/Helpers`), `Resources/SceneHost` beside the binary on
/// Windows and Linux. Otherwise it is the crate's `resources` folder, which packaging copies.
pub fn resources() -> PathBuf {
    let holds = |folder: &std::path::Path| folder.join("vendor").is_dir();
    if let Some(packaged) = std::env::current_exe()
        .ok()
        .and_then(|program| program.canonicalize().ok())
        .and_then(|program| {
            let folder = program.parent()?;
            [
                folder.join("../Resources/SceneHost"),
                folder.join("Resources/SceneHost"),
            ]
            .into_iter()
            .find(|folder| holds(folder))
        })
    {
        return packaged.canonicalize().unwrap_or(packaged);
    }
    let source = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/resources"));
    source.canonicalize().unwrap_or(source)
}

/// Writes a timestamped trace line to stderr when `NR_HOST_DEBUG` is set (`nr_host_trace`).
pub fn trace(line: impl FnOnce() -> String) {
    if nr::nr_host_tracing() {
        let line = c(&line());
        // SAFETY: a NUL-terminated string.
        unsafe { nr::nr_host_trace(line.as_ptr()) };
    }
}

pub fn tracing() -> bool {
    nr::nr_host_tracing()
}

/// The live page script that records its errors in the log.
pub fn page_errors_script() -> String {
    text(nr::nr_host_page_errors_script())
}

/// The call that tells a live page a database it queried changed (`HostCore::data_changed`).
pub fn data_changed_call() -> String {
    text(nr::nr_host_data_changed_call())
}

pub fn scheme() -> String {
    text(nr::nr_host_scheme())
}

/// Where the core keeps the compiled network block list.
pub fn rules_folder() -> String {
    text(nr::nr_host_rules_folder())
}

pub fn block_rules_name() -> String {
    text(nr::nr_host_block_rules_name())
}

pub fn block_rules() -> String {
    text(nr::nr_host_block_rules())
}

pub fn ready_timeout_seconds() -> f64 {
    nr::nr_host_ready_timeout_seconds()
}

pub fn start_timeout_seconds() -> f64 {
    nr::nr_host_start_timeout_seconds()
}

/// Side length of a model still's square stage, in pixels.
pub fn still_stage() -> u32 {
    nr::nr_host_still_stage()
}

/// Recovers a transparent picture from renders over black and over white (premultiplied RGBA,
/// same size). The core does the arithmetic (`host::unground`).
pub fn unground(black: &[u8], white: &[u8]) -> Vec<u8> {
    let mut out = vec![0; black.len()];
    // SAFETY: three buffers of the same length.
    unsafe { nr::nr_pixels_unground(black.as_ptr(), white.as_ptr(), out.as_mut_ptr(), out.len()) };
    out
}

/// The core's page host for one web view (`core/src/page/host`). It answers the scheme, decides
/// navigations, reads page messages and keeps the log. Safe to call from any thread.
pub struct HostCore(*mut nr::NrHost);

// SAFETY: the core's host is thread-safe; its log sits behind a lock.
unsafe impl Send for HostCore {}
// SAFETY: as above.
unsafe impl Sync for HostCore {}

/// How a host's web view plays media and what its pages may open (`host::Kind::web`).
pub struct Web {
    pub video_plays_alone: bool,
    pub sound_plays_alone: bool,
    pub opens_windows: bool,
    pub fullscreen: bool,
    pub airplay: bool,
}

/// The response for one request, with the file range the core named already read.
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// `None` when the file the core named could not be read.
    pub body: Option<Vec<u8>>,
}

impl HostCore {
    /// A host serving `file` as a live page over its folder.
    pub fn page(file: &str) -> Arc<Self> {
        let (file, resources) = (c(file), c(&resources().to_string_lossy()));
        // SAFETY: NUL-terminated strings.
        Arc::new(Self(unsafe {
            nr::nr_host_open_page(file.as_ptr(), resources.as_ptr())
        }))
    }

    /// A host for the page that draws 3D model stills (`.glb`), fed one at a time by `draw_model`.
    pub fn stills() -> Arc<Self> {
        let resources = c(&resources().to_string_lossy());
        // SAFETY: a NUL-terminated string.
        Arc::new(Self(unsafe { nr::nr_host_open_stills(resources.as_ptr()) }))
    }

    /// Queues `file` on a still host. Returns the script call that draws it and the turn the
    /// page's answer will carry.
    pub fn draw_model(&self, file: &str) -> (String, u64) {
        let file = c(file);
        // SAFETY: a live still host and a NUL-terminated string.
        let turn = unsafe { nr::nr_host_draw_model(self.0, file.as_ptr()) };
        (text(turn.call), turn.turn)
    }

    pub fn web(&self) -> Web {
        // SAFETY: a live host.
        let web = unsafe { nr::nr_host_web(self.0) };
        Web {
            video_plays_alone: web.video_plays_alone,
            sound_plays_alone: web.sound_plays_alone,
            opens_windows: web.opens_windows,
            fullscreen: web.fullscreen,
            airplay: web.airplay,
        }
    }

    /// Whether a live page reloads when its web content process ends.
    pub fn crash_reloads(&self) -> bool {
        // SAFETY: a live host.
        unsafe { nr::nr_host_crash_reloads(self.0) }
    }

    /// Whether a database the page queried was changed by someone else since its last query.
    /// Reports each change once.
    pub fn data_changed(&self) -> bool {
        // SAFETY: a live host.
        unsafe { nr::nr_host_data_changed(self.0) }
    }

    /// The address the web view loads first.
    pub fn entry(&self) -> String {
        // SAFETY: a live host.
        text(unsafe { nr::nr_host_entry(self.0) })
    }

    /// The response for `url`, given its `Range` header (empty when none).
    pub fn answer(&self, url: &str, range: &str) -> Reply {
        let (url, range) = (c(url), c(range));
        // SAFETY: a live host and NUL-terminated strings.
        Self::reply(unsafe { nr::nr_host_answer(self.0, url.as_ptr(), range.as_ptr()) })
    }

    fn reply(answer: *mut nr::NrHostAnswer) -> Reply {
        // SAFETY: a live answer, freed once read.
        let view = unsafe { nr::nr_host_reply(answer) };
        let headers: BTreeMap<String, String> = state(&text(view.headers));
        let file = text(view.file);
        let body = if file.is_empty() {
            Some(if view.body.is_null() {
                Vec::new()
            } else {
                // SAFETY: the answer holds `body_length` bytes until it is freed.
                unsafe { std::slice::from_raw_parts(view.body, view.body_length) }.to_vec()
            })
        } else {
            read_stretch(&file, view.offset, view.length)
        };
        let reply = Reply {
            status: view.status,
            headers: headers.into_iter().collect(),
            body,
        };
        // SAFETY: the answer is not used again.
        unsafe { nr::nr_host_answer_free(answer) };
        reply
    }

    /// Whether the web view may navigate to `url`. Blocked navigations are logged.
    pub fn navigate(&self, url: &str, main_frame: bool, download: bool) -> bool {
        let url = c(url);
        // SAFETY: a live host and a NUL-terminated string.
        unsafe { nr::nr_host_navigate(self.0, url.as_ptr(), main_frame, download) }
    }

    /// Handles a message a page posted, returning the page a `reloaded` message names.
    pub fn page_message(&self, body: &str) -> Option<String> {
        let body = c(body);
        // SAFETY: a live host and a NUL-terminated string.
        let reloaded = text(unsafe { nr::nr_host_page_message(self.0, body.as_ptr()) });
        (!reloaded.is_empty()).then_some(reloaded)
    }

    fn log(&self) -> LogState {
        // SAFETY: a live host.
        state(&text(unsafe { nr::nr_host_log(self.0) }))
    }

    /// Takes the next new log line as `(kind, text)`.
    fn take_line(&self) -> Option<(String, String)> {
        // SAFETY: a live host.
        let taken = unsafe { nr::nr_host_take_line(self.0) };
        let kind = text(taken.kind);
        (!kind.is_empty()).then(|| (kind, text(taken.line)))
    }

    fn clear_log(&self) {
        // SAFETY: a live host.
        unsafe { nr::nr_host_clear_log(self.0) }
    }
}

impl Drop for HostCore {
    fn drop(&mut self) {
        // SAFETY: the host is not used again.
        unsafe { nr::nr_host_close(self.0) }
    }
}

fn read_stretch(path: &str, offset: u64, length: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut body = Vec::new();
    file.take(length).read_to_end(&mut body).ok()?;
    Some(body)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogState {
    blocked: Vec<String>,
    refused: Vec<String>,
    missing: Vec<String>,
    errors: Vec<String>,
    fonts: Vec<String>,
    served: Vec<String>,
    requests: u64,
    range_requests: u64,
}

/// Receives each new log line as `(kind, text)`.
type LineHandler = Box<dyn Fn(&str, &str)>;

/// A readable record of what pages requested and did not get. The core keeps it; this reads it.
pub struct HostLog {
    core: Arc<HostCore>,
    read: RefCell<Option<LogState>>,
    /// Called for each new line. Kinds are "blocked", "refused", "missing", "error" and "font".
    on_line: RefCell<Option<LineHandler>>,
    lines: Cell<u64>,
}

impl HostLog {
    pub(crate) fn new(core: Arc<HostCore>) -> Self {
        Self {
            core,
            read: RefCell::new(None),
            on_line: RefCell::new(None),
            lines: Cell::new(0),
        }
    }

    fn state(&self) -> LogState {
        if let Some(read) = self.read.borrow().as_ref() {
            return read.clone();
        }
        let state = self.core.log();
        *self.read.borrow_mut() = Some(state.clone());
        state
    }

    /// Blocked network attempts, such as "fetch https://example.com/ping (scenes/a.html)".
    pub fn blocked(&self) -> Vec<String> {
        self.state().blocked
    }

    /// Refused requests for files outside the page's folder.
    pub fn refused(&self) -> Vec<String> {
        self.state().refused
    }

    /// Requested files inside the page's folder that do not exist.
    pub fn missing(&self) -> Vec<String> {
        self.state().missing
    }

    /// Problems pages reported, such as script errors, failed loads and late frames.
    pub fn page_errors(&self) -> Vec<String> {
        self.state().errors
    }

    /// Google Fonts families a page requested that are neither packaged nor in `fonts/`.
    pub fn missing_fonts(&self) -> Vec<String> {
        self.state().fonts
    }

    pub fn requests(&self) -> u64 {
        self.state().requests
    }

    pub fn range_requests(&self) -> u64 {
        self.state().range_requests
    }

    /// Folder-relative paths of every file served to the pages.
    pub fn served(&self) -> std::collections::BTreeSet<String> {
        self.state().served.into_iter().collect()
    }

    pub fn on_line(&self, handler: impl Fn(&str, &str) + 'static) {
        *self.on_line.borrow_mut() = Some(Box::new(handler));
    }

    /// Number of lines delivered so far.
    pub fn lines(&self) -> u64 {
        self.lines.get()
    }

    /// Passes lines added since the last call to `on_line`. Called on the main thread after every
    /// change to the log.
    pub(crate) fn deliver(&self) {
        *self.read.borrow_mut() = None;
        while let Some((kind, line)) = self.core.take_line() {
            self.lines.set(self.lines.get() + 1);
            if let Some(handler) = self.on_line.borrow().as_ref() {
                handler(&kind, &line);
            }
        }
    }

    pub fn clear(&self) {
        self.deliver();
        self.core.clear_log();
        *self.read.borrow_mut() = None;
    }
}
