//! The boundary every shell uses (ADR-0001): commands in, view state out. Pixels
//! never cross it: a shell passes a native layer and the core draws.
//!
//! Strings the core hands out are borrowed UTF-8, not NUL-terminated, valid until the call that
//! documents otherwise.

use crate::columns::Columns;
use crate::preview::Preview;
use crate::terminal::launch::{build_launch, choose_shell};
use crate::terminal::{Palette, Terminal};
use crate::theme::{derive, oklch_to_srgb8, Appearance, Mode, Token};
use crate::workspace::{EntryKind, Workspace};
use crate::Relock;
use crate::{app, start};
use std::ffi::{c_char, c_void, CStr};
use std::path::{Path, PathBuf};

/// Borrowed UTF-8 text.
#[repr(C)]
pub struct NrText {
    pub bytes: *const u8,
    pub length: usize,
}

impl NrText {
    pub(crate) fn borrowing(text: &str) -> Self {
        Self {
            bytes: text.as_ptr(),
            length: text.len(),
        }
    }

    /// Lends `text` from `slot`, which holds it until the slot's next text. Text lent through a
    /// function's own slot stays valid until that function lends again.
    pub(crate) fn lend_into(slot: &'static std::sync::Mutex<String>, text: String) -> Self {
        let mut lent = slot.relock();
        *lent = text;
        Self::borrowing(&lent)
    }

    /// Copies the text. An empty text may have a null `bytes`, and reads as an empty string.
    ///
    /// # Safety
    /// Unless `length` is 0, `bytes` must point to `length` bytes that are alive now.
    pub unsafe fn to_owned_string(&self) -> String {
        if self.length == 0 {
            return String::new();
        }
        // SAFETY: the caller passes `length` live bytes at `bytes`.
        String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(self.bytes, self.length) })
            .into_owned()
    }
}

// MARK: Baselines

/// Where a line of a browser's own media controls puts its baseline, for a face of `family`
/// (`baseline::controls`).
///
/// # Safety
/// `family` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_baseline(
    top: f64,
    line_height: f64,
    ascent: f64,
    descent: f64,
    family: *const c_char,
    scale: f64,
) -> f64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let family = unsafe { text_of(family) };
    crate::text::baseline::controls(top, line_height, ascent, descent, &family, scale)
}

/// How far below a line's top the open page sets its baseline (`baseline::page`).
#[no_mangle]
pub extern "C" fn nr_page_baseline(line_height: f64, ascent: f64, descent: f64) -> f64 {
    crate::text::baseline::page(line_height, ascent, descent)
}

// MARK: Wheels

/// Wheel travel in points, as from a trackpad.
pub const NR_WHEEL_POINTS: u32 = 0;
/// Wheel travel in clicks, as from a mouse wheel that turns in steps.
pub const NR_WHEEL_CLICKS: u32 = 3;

// MARK: Keys

/// ⌘ held, as a bit of a `held` mask. Every view and a check's steps use these bits.
pub const NR_HELD_COMMAND: u32 = 1;
/// Control held.
pub const NR_HELD_CONTROL: u32 = 2;
/// ⌥ held.
pub const NR_HELD_OPTION: u32 = 4;
/// ⇧ held.
pub const NR_HELD_SHIFT: u32 = 8;

/// A key with no character of its own, as every view and a check's steps name it. A shell
/// maps its platform's key codes to these with one table; keypad Enter is `Return`. Any other
/// key is `Other`, and what it types is in `NrKeyPress`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrKey {
    Other,
    Return,
    Tab,
    Space,
    Escape,
    /// The key above Return.
    Backspace,
    /// The forward delete key.
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    /// Insert, the Help key on a Mac keyboard.
    Insert,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl NrKey {
    /// F1 to F12 by number.
    pub const FUNCTIONS: [Self; 12] = [
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
    ];

    /// The number of a function key, 1 to 12.
    pub fn function(self) -> Option<u8> {
        Self::FUNCTIONS
            .iter()
            .position(|key| *key == self)
            .map(|index| index as u8 + 1)
    }
}

/// A key press. `key` names a key with no character of its own. `character` is what it types
/// without ⌘ or Control (0 for none). `letter` is the letter or digit at that place on the US
/// layout, whatever the actual layout (0 for none), because the page's shortcuts go by place.
/// `held` is the `NR_HELD_*` mask.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NrKeyPress {
    pub key: NrKey,
    pub character: u32,
    pub letter: u32,
    pub held: u32,
}

impl NrKeyPress {
    /// `key` with nothing held.
    pub const fn named(key: NrKey) -> Self {
        Self {
            key,
            character: 0,
            letter: 0,
            held: 0,
        }
    }

    pub fn shift(self) -> bool {
        self.held & NR_HELD_SHIFT != 0
    }

    /// Whether ⌘, Control or ⌥ is held.
    pub fn modified(self) -> bool {
        self.held & (NR_HELD_COMMAND | NR_HELD_CONTROL | NR_HELD_OPTION) != 0
    }

    /// What it types, for a key with no name.
    pub fn typed(self) -> Option<char> {
        if self.key != NrKey::Other || self.character == 0 {
            return None;
        }
        char::from_u32(self.character)
    }

    /// The US layout's letter or digit of a key with no name, in lower case.
    pub fn letter(self) -> Option<char> {
        if self.key != NrKey::Other || self.letter == 0 {
            return None;
        }
        char::from_u32(self.letter).map(|letter| letter.to_ascii_lowercase())
    }
}

#[repr(C)]
pub struct NrPreviewInfo {
    pub duration_seconds: f64,
    pub position_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub playing: bool,
    pub hdr: bool,
    pub ready: bool,
    /// Playback reached the end and stopped.
    pub ended: bool,
    pub has_audio: bool,
    pub muted: bool,
    pub volume: f32,
    /// The picture fills the screen (`Preview::ask_fullscreen`).
    pub fullscreen: bool,
    /// How fast it plays: 1 is the file's own speed.
    pub rate: f64,
    /// It has played since the file was opened.
    pub has_played: bool,
    /// Sound output should run because a file with sound is playing
    /// (`PreviewInfo::output_wanted`).
    pub output_wanted: bool,
}

/// Starts the core.
#[no_mangle]
pub extern "C" fn nr_app_start() {
    start();
}

/// Shows the preview on `layer`, a `CAMetalLayer` sized `width` × `height` pixels. The shell
/// keeps the layer alive until `nr_preview_stop` returns. Off a Mac `layer` is null: the
/// preview presents into a picture that size, which the shell draws (`nr_preview_picture`).
///
/// # Safety
/// On a Mac `layer` must be a live `CAMetalLayer`.
#[no_mangle]
pub unsafe extern "C" fn nr_preview_attach(layer: *mut c_void, width: u32, height: u32) {
    with_preview(|preview| preview.attach(layer, width, height));
}

/// The preview's last presented frame off a Mac (`preview::Picture`): `width` × `height` BGRA
/// pixels in sRGB, row by row, with its frame number.
#[cfg(not(target_os = "macos"))]
#[repr(C)]
pub struct NrPreviewPicture {
    pub width: u32,
    pub height: u32,
    pub number: u64,
    pub bgra: *const u8,
}

/// Fills `out` with the preview's frame last presented, off a Mac; false before the first. The
/// pixels are lent until the next call.
///
/// # Safety
/// `out` must point to writable storage for one `NrPreviewPicture`.
#[cfg(not(target_os = "macos"))]
#[no_mangle]
pub unsafe extern "C" fn nr_preview_picture(out: *mut NrPreviewPicture) -> bool {
    static LENT: std::sync::Mutex<Option<std::sync::Arc<crate::preview::Picture>>> =
        std::sync::Mutex::new(None);
    let Some(picture) = with_preview(crate::preview::Preview::picture).flatten() else {
        return false;
    };
    // SAFETY: the caller passes storage for one picture.
    unsafe {
        out.write(NrPreviewPicture {
            width: picture.width,
            height: picture.height,
            number: picture.number,
            bgra: picture.bgra.as_ptr(),
        });
    }
    *LENT.relock() = Some(picture);
    true
}

/// The last number a file opened in the preview took; each takes the next.
static PREVIEW_OPENED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The number of the file the preview holds; 0 when it holds none.
static PREVIEW_HELD: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// The number of the file a page's player woke into the preview; 0 when none is awake.
static PAGE_PREVIEW: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Puts what `start` opens in the preview, in place of whatever it held, and returns the
/// number its opener closes it by (`nr_preview_close`).
fn hold_preview(start: impl FnOnce() -> Preview) -> u64 {
    use std::sync::atomic::Ordering;
    nr_preview_stop();
    let preview = start();
    let number = PREVIEW_OPENED.fetch_add(1, Ordering::Relaxed) + 1;
    *app().preview.relock() = Some(preview);
    PREVIEW_HELD.store(number, Ordering::Relaxed);
    number
}

pub(crate) fn with_preview<T>(action: impl FnOnce(&Preview) -> T) -> Option<T> {
    let preview = app().preview.relock();
    preview.as_ref().map(action)
}

/// Starts the preview playing: what a film or video opened by its card's play button does
/// (`nr_opened_takes_play`).
#[no_mangle]
pub extern "C" fn nr_preview_play() {
    with_preview(|preview| preview.set_playing(true));
}

/// The window left full screen by a system route (the green button, ⌃⌘F, Mission Control), so
/// the picture stops filling the screen too.
#[no_mangle]
pub extern "C" fn nr_preview_leave_fullscreen() {
    with_preview(|preview| {
        preview.ask_fullscreen(crate::preview::player::video_controls::FullscreenAsk::Leave);
    });
}

#[no_mangle]
pub extern "C" fn nr_preview_resize(width: u32, height: u32) {
    with_preview(|preview| preview.resize(width, height));
}

#[no_mangle]
pub extern "C" fn nr_preview_info() -> NrPreviewInfo {
    let info = with_preview(Preview::info).unwrap_or_default();
    NrPreviewInfo {
        duration_seconds: info.duration_seconds,
        position_seconds: info.position_seconds,
        width: info.width,
        height: info.height,
        playing: info.playing,
        hdr: info.hdr,
        ready: info.ready,
        ended: info.ended,
        has_audio: info.has_audio,
        muted: info.muted,
        volume: info.volume,
        fullscreen: info.fullscreen,
        rate: info.rate,
        has_played: info.has_played,
        output_wanted: info.output_wanted(),
    }
}

/// Opens the video file at `path` in the preview, in place of whatever it held, paused on
/// its first frame. Nothing is drawn until `nr_preview_attach`. Returns the number its opener
/// closes it by (`nr_preview_close`).
///
/// # Safety
/// `path` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_preview_open(path: *const c_char) -> u64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let path = unsafe { text_of(path) };
    hold_preview(|| Preview::start_as(path, crate::preview::Playing::Video, 0.0))
}

/// Opens the sound file at `path` in the preview, in place of whatever it held, paused at its
/// start. It has no picture and takes no layer. Returns the number its opener closes it by
/// (`nr_preview_close`).
///
/// # Safety
/// `path` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_preview_open_sound(path: *const c_char) -> u64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let path = unsafe { text_of(path) };
    hold_preview(|| Preview::start_as(path, crate::preview::Playing::Sound, 0.0))
}

/// Fills `out` with the next `frames` frames of the preview's sound, interleaved in the output's
/// channels, 32-bit float at its rate (`nr_film_sound_output`), taken at the preview's clock;
/// silence when it is paused, muted or has no sound. The shell's audio output calls this from
/// its own thread.
///
/// # Safety
/// `out` must point to writable storage for `frames * channels` floats.
#[no_mangle]
pub unsafe extern "C" fn nr_preview_audio_render(out: *mut f32, frames: u32) {
    // SAFETY: the caller passes storage for one float per channel per frame.
    let out = unsafe {
        std::slice::from_raw_parts_mut(
            out,
            frames as usize * crate::preview::audio::OUTPUT.channels as usize,
        )
    };
    // Everyone who holds this lock holds it for a few instructions, so the output waits at
    // most that long; skipping a turn instead would be heard as a gap.
    if with_preview(|preview| preview.render_audio(out)).is_none() {
        out.fill(0.0);
    }
}

#[no_mangle]
pub extern "C" fn nr_preview_stop() {
    let preview = app().preview.relock().take();
    PREVIEW_HELD.store(0, std::sync::atomic::Ordering::Relaxed);
    if let Some(preview) = preview {
        preview.stop();
    }
}

/// Stops the preview if it still holds the file opened as `number` (`nr_preview_open`) and
/// returns whether it did. A file opened since is left playing, so a screen that goes away can
/// call this after the next screen has opened its own file.
#[no_mangle]
pub extern "C" fn nr_preview_close(number: u64) -> bool {
    let held = number != 0 && PREVIEW_HELD.load(std::sync::atomic::Ordering::Relaxed) == number;
    if held {
        nr_preview_stop();
    }
    held
}

/// Closes the file a page's player woke, when the preview still holds it.
fn close_page_preview() {
    nr_preview_close(PAGE_PREVIEW.swap(0, std::sync::atomic::Ordering::Relaxed));
}

/// The session of the terminal on screen as `number` (`nr_terminal_attach`).
pub(crate) fn with_shown<T>(number: u64, action: impl FnOnce(&Terminal) -> T) -> Option<T> {
    let app = app();
    let id = app.shown_terminals.relock().file(number)?.to_owned();
    let sessions = app.sessions.relock();
    sessions.get(&id).map(action)
}

/// The number of the terminal that has the keyboard, or was shown last; 0 for none.
pub(crate) fn front_terminal() -> u64 {
    app().shown_terminals.relock().front()
}

/// The terminal's text size in points: the shell measures its terminal font at this size
/// (`nr_terminal_cell`).
#[no_mangle]
pub extern "C" fn nr_terminal_font_points() -> f32 {
    crate::terminal::frame::FONT_POINTS
}

/// A terminal cell in whole pixels, with its baseline and line thickness.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NrTerminalCell {
    pub width: f32,
    pub height: f32,
    pub baseline: f32,
    pub stroke: f32,
}

/// The cell for the terminal font as the shell measures it: its `ascent`, `descent` and a
/// character's `advance`, in pixels at `nr_terminal_font_points` and `scale` pixels per point.
#[no_mangle]
pub extern "C" fn nr_terminal_cell(
    ascent: f32,
    descent: f32,
    advance: f32,
    scale: f32,
) -> NrTerminalCell {
    let cell = crate::terminal::frame::CellMetrics::of_font(
        crate::terminal::frame::FontMetrics {
            ascent,
            descent,
            advance,
        },
        scale,
    );
    NrTerminalCell {
        width: cell.width,
        height: cell.height,
        baseline: cell.baseline,
        stroke: cell.stroke,
    }
}

/// The view a terminal is drawn in: `width` × `height` pixels, and its cell
/// (`nr_terminal_cell`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NrTerminalView {
    pub width: u32,
    pub height: u32,
    pub cell: NrTerminalCell,
}

impl NrTerminalView {
    fn size(self) -> crate::terminal::ViewSize {
        crate::terminal::ViewSize {
            width: self.width.max(1),
            height: self.height.max(1),
            cell: crate::terminal::frame::CellMetrics {
                width: self.cell.width.max(1.0),
                height: self.cell.height.max(1.0),
                baseline: self.cell.baseline,
                stroke: self.cell.stroke,
            },
        }
    }
}

/// Shows the session of the terminal file that is open in `view`, starting its shell when none
/// runs, and returns the number the terminal is known by on screen: every other
/// `nr_terminal_*` call names it, and `nr_terminal_detach` takes it off. 0 when no terminal
/// file is open. The shell draws its frames (`nr_terminal_frame`).
#[no_mangle]
pub extern "C" fn nr_terminal_attach(view: NrTerminalView) -> u64 {
    let Some((id, path)) = opened_terminal_file() else {
        return 0;
    };
    show_session(&id, &path, view.size())
}

/// As `nr_terminal_attach`, for the terminal file `id` of the drive wherever it is: one of a
/// folder's terminals tiled across the window. 0 when no such file is on the drive.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_attach_file(id: *const c_char, view: NrTerminalView) -> u64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    let Some(path) = with_columns(|columns| columns.workspace().path_of(&id).ok()).flatten() else {
        return 0;
    };
    show_session(&id, &path, view.size())
}

/// The terminal colours of the theme in effect: its ground, text and accent surface.
fn terminal_palette() -> Palette {
    let theme = app().theme.relock();
    Palette::on(
        oklch_to_srgb8(theme.background),
        oklch_to_srgb8(theme.foreground),
        oklch_to_srgb8(theme.accent),
    )
}

/// Shows the session of the terminal file `id` at `path` in `view`, starting its shell in
/// the file's folder when none runs, and returns the number it is known by on screen.
fn show_session(id: &str, path: &Path, view: crate::terminal::ViewSize) -> u64 {
    let app = app();
    let environment: Vec<(String, String)> = std::env::vars().collect();
    let palette = terminal_palette();
    // Read before the sessions' lock, which the columns' lock never waits under.
    let (drive, metadata) = with_columns(|columns| {
        let workspace = columns.workspace();
        (
            workspace.root().to_path_buf(),
            workspace.metadata_directory().to_path_buf(),
        )
    })
    .unzip();
    // The terminal file decides the folder; the person decides what runs (ADR-0005).
    let mut sessions = app.sessions.relock();
    match sessions.get(id) {
        // Already running: show it as it was left.
        Some(session) => session.attach(view),
        None => {
            let shell = choose_shell(std::env::var("SHELL").ok().as_deref());
            let directory = path
                .parent()
                .map_or_else(|| PathBuf::from("/"), Path::to_path_buf);
            // Agents started in this session learn how to report activity. The hook files
            // live in the app's data; if writing them fails, the session runs unreported.
            let harness_home = crate::paths::get().support;
            let session = match crate::terminal::harness::write_harness_files(&harness_home) {
                Ok(plugin) => crate::terminal::harness::session_environment(&plugin),
                Err(error) => {
                    eprintln!("the terminal's activity hooks were not written: {error}");
                    Vec::new()
                }
            };
            let launch = build_launch(&directory, &environment, &shell, &session);
            let turns =
                crate::changes::ffi::follow_terminal(id, path, &directory, drive.as_deref());
            // The terminal's output from before the last quit shows above the new shell, once.
            let restored = metadata
                .as_deref()
                .and_then(|metadata| crate::terminal::transcript::take(metadata, id))
                .map(|text| crate::terminal::transcript::shown(&text, interface_language()))
                .unwrap_or_default();
            let started = Terminal::start(view, launch, palette, turns, &restored);
            // An agent open in it at the last quit opens its conversation again: the command is
            // typed once the shell waits at its prompt.
            let resume = metadata
                .as_deref()
                .and_then(|metadata| crate::terminal::transcript::take_resume(metadata, id));
            if let Some(command) = resume {
                started.type_ahead(format!("{}\r", command.trim()).into_bytes());
            }
            sessions.insert(id.to_owned(), started);
        }
    }
    app.shown_terminals.relock().show(id)
}

/// Takes the terminal shown as `number` off screen. Its session runs on, and is marked off
/// screen unless another screen shows it now (the next tab's, which took it over).
#[no_mangle]
pub extern "C" fn nr_terminal_detach(number: u64) {
    let app = app();
    let gone = app.shown_terminals.relock().hide(number);
    if let Some(id) = gone {
        let sessions = app.sessions.relock();
        if let Some(session) = sessions.get(&id) {
            session.detach();
        }
    }
}

/// The id and path of the terminal file filling the window, when one is.
fn opened_terminal_file() -> Option<(String, PathBuf)> {
    let mut columns = app().columns.relock();
    let columns = columns.as_mut()?;
    let opened = columns.opened()?.clone();
    if opened.kind != EntryKind::Terminal {
        return None;
    }
    let path = columns.workspace().path_of(&opened.id).ok()?;
    Some((opened.id, path))
}

/// What a file of the drive is.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrFileKind {
    Folder,
    Timeline,
    Page,
    Terminal,
    Image,
    Video,
    Audio,
    /// Any other file.
    File,
}

/// One row of a drive column. `holds_files` is true for a folder or a timeline: its selection
/// opens the next column, and the list opens it in place. For a terminal, `terminal::activity`
/// is 0 not running, 1 idle, 2 starting, 3 working, 4 needs you, 5 finished, 6 ended.
///
/// `id` names the file in every command that takes ids.
#[repr(C)]
pub struct NrFileRow {
    pub title: NrText,
    pub kind: NrFileKind,
    pub holds_files: bool,
    pub terminal_activity: u32,
    pub id: NrText,
    /// A file of source code, which is marked with the code glyph.
    pub code: bool,
    /// What files dragged within the drive can be dropped on: a folder, to move into it, or a
    /// terminal whose program reads them (`takes_references`).
    pub takes_drops: bool,
    /// A terminal whose program is running and reads what is pasted: files dropped on it are
    /// handed to it as references (`reference_files`).
    pub takes_references: bool,
    /// The name, among the app's pictures, of the mark of the agent started in a terminal;
    /// empty for a terminal with none, which shows the terminal glyph, and for anything else.
    pub terminal_logo: NrText,
    /// The glyph shown where no picture of the file is drawn, by its name (`glyph::Glyph`).
    pub glyph: NrText,
}

/// One file as a shell's row. The text borrows from `entry`.
pub(crate) fn file_row(entry: &crate::workspace::Entry, running: &[RunningSession]) -> NrFileRow {
    file_row_with(entry, running.iter().find(|session| session.id == entry.id))
}

/// One row of a file, as `file_row` makes it, with `session` the one running in it, if any.
pub(crate) fn file_row_with(
    entry: &crate::workspace::Entry,
    session: Option<&RunningSession>,
) -> NrFileRow {
    let activity = session.map_or(0, |session| session.activity);
    let takes_references = entry.kind == EntryKind::Terminal
        && crate::terminal::activity::Activity::numbered(activity)
            .is_some_and(crate::terminal::activity::Activity::takes_references);
    NrFileRow {
        title: NrText::borrowing(&entry.title),
        kind: file_kind(entry.kind),
        holds_files: entry.kind.holds_files(),
        terminal_activity: activity,
        id: NrText::borrowing(&entry.id),
        code: entry.family == crate::workspace::FileFamily::Code,
        takes_drops: entry.kind == EntryKind::Folder || takes_references,
        takes_references,
        terminal_logo: NrText::borrowing(session.map_or("", RunningSession::logo)),
        glyph: NrText::borrowing(crate::list::file_glyph(entry.kind, entry.family).name()),
    }
}

/// A session running: its terminal file's id, its activity as a row's number, and the agent
/// the person has started in it.
pub(crate) struct RunningSession {
    pub id: String,
    pub activity: u32,
    pub agent: Option<crate::terminal::agent::Agent>,
}

impl RunningSession {
    /// The name of its agent's mark among the app's pictures; empty with no agent, or one
    /// whose mark is not shown.
    pub fn logo(&self) -> &'static str {
        self.agent
            .and_then(crate::terminal::agent::Agent::logo)
            .unwrap_or("")
    }
}

/// Every session running.
pub(crate) fn running_sessions() -> Vec<RunningSession> {
    app()
        .sessions
        .relock()
        .iter()
        .map(|(id, session)| RunningSession {
            id: id.clone(),
            activity: session.activity().number(),
            agent: session.agent(),
        })
        .collect()
}

/// The session running in the terminal file `id`, if any. It takes one look under the lock and
/// copies nothing when none runs, because rows call it every frame.
pub(crate) fn running_session(id: &str) -> Option<RunningSession> {
    app()
        .sessions
        .relock()
        .get(id)
        .map(|session| RunningSession {
            id: id.to_owned(),
            activity: session.activity().number(),
            agent: session.agent(),
        })
}

/// How a terminal doing a row's `terminal::activity` looks (`terminal::activity::look`), as theme
/// colour names: the tile's ring at `ring_opacity`, the corner dot (empty for none) at its own
/// opacity times `dot_fade`, and the word under a card's name.
#[repr(C)]
pub struct NrTerminalLook {
    pub ring: NrText,
    pub ring_opacity: f64,
    pub dot: NrText,
    pub dot_fade: f64,
    /// The shape the dot takes (`terminal::activity::Sign::name`).
    pub sign: NrText,
    pub label: NrText,
}

#[no_mangle]
pub extern "C" fn nr_terminal_look(activity: u32) -> NrTerminalLook {
    let look =
        crate::terminal::activity::look(crate::terminal::activity::Activity::numbered(activity));
    NrTerminalLook {
        ring: NrText::borrowing(look.ring.name()),
        ring_opacity: look.ring_opacity,
        dot: NrText::borrowing(look.dot.map_or("", Token::name)),
        dot_fade: look.dot_fade,
        sign: NrText::borrowing(look.sign.name()),
        label: NrText::borrowing(look.label.name()),
    }
}

/// Lends a text the core just made, valid until the next call that lends one.
fn lend(text: String) -> NrText {
    static LENT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    NrText::lend_into(&LENT, text)
}

/// Ids as a shell passes several: one per line.
pub(crate) fn ids_of(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The file filling the window beside the columns. `kind` as in `NrFileRow`.
#[repr(C)]
pub struct NrOpened {
    pub title: NrText,
    pub kind: NrFileKind,
}

/// A colour as gamma-encoded sRGB with straight alpha, each 0–1.
#[repr(C)]
pub struct NrColor {
    pub red: f32,
    pub green: f32,
    pub blue: f32,
    pub alpha: f32,
}

pub(crate) fn file_kind(kind: EntryKind) -> NrFileKind {
    match kind {
        EntryKind::Folder => NrFileKind::Folder,
        EntryKind::Page => NrFileKind::Page,
        EntryKind::Terminal => NrFileKind::Terminal,
        EntryKind::Image => NrFileKind::Image,
        EntryKind::Video => NrFileKind::Video,
        EntryKind::Audio => NrFileKind::Audio,
        EntryKind::File => NrFileKind::File,
    }
}

/// A string the shell passed, copied; a null pointer reads as an empty string.
///
/// # Safety
/// `text` is null or a NUL-terminated string.
pub(crate) unsafe fn text_of(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: the caller passes a NUL-terminated string.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn with_columns<T>(action: impl FnOnce(&mut Columns) -> T) -> Option<T> {
    let mut columns = app().columns.relock();
    columns.as_mut().map(action)
}

/// Goes up whenever a column or the open file changes. A shell redraws when it differs from
/// the value it last drew.
#[no_mangle]
pub extern "C" fn nr_columns_revision() -> u64 {
    // Opening, closing or switching settings redraws the window as a folder change does.
    let settings = app().settings.relock().1;
    with_columns(|columns| columns.revision()).unwrap_or(0) + settings
}

/// The interface language code. English only (ADR-0001), because a partly translated catalog
/// shows mostly English with bits of the person's own language mixed in. Adding a language
/// means adding its catalog (`strings/<code>.json`, `build.rs`) and choosing it here from the
/// system's preferred languages or Settings. Dates and money follow the person's region
/// regardless (`format`).
pub(crate) const fn interface_language() -> &'static str {
    crate::strings::LANGUAGES[0]
}

/// Opens the diagnostic log in the app's logs folder (`paths::Paths::logs`) and writes its two
/// opening lines (`diagnostics::start`), which say where it is and what is running, using facts
/// only the platform knows (`startup`). If the folder cannot be used, the app runs without a log
/// and says so on the error output.
pub fn log_start(startup: &crate::diagnostics::Startup) {
    let paths = crate::paths::get();
    crate::diagnostics::start(&paths.logs, &paths.support, startup);
}

/// The folder the diagnostic log is in, which Help → Open Diagnostic Logs opens. Empty when
/// the app has no log.
#[no_mangle]
pub extern "C" fn nr_log_directory() -> NrText {
    lend(
        crate::diagnostics::directory().map_or_else(String::new, |directory| {
            directory.to_string_lossy().into_owned()
        }),
    )
}

/// Settings' page while settings are open (`settings::page::TAB_APPEARANCE`), or -1 when they
/// are closed.
#[no_mangle]
pub extern "C" fn nr_settings_tab() -> i32 {
    app().settings.relock().0.map_or(-1, |tab| tab as i32)
}

/// Opens settings, in place of the folder.
#[no_mangle]
pub extern "C" fn nr_settings_open() {
    let mut settings = app().settings.relock();
    let tab = crate::settings::page::TAB_APPEARANCE;
    if settings.0 != Some(tab) {
        *settings = (Some(tab), settings.1 + 1);
    }
}

/// Closes settings; the folder shows as it was left.
#[no_mangle]
pub extern "C" fn nr_settings_close() {
    let mut settings = app().settings.relock();
    if settings.0.is_some() {
        *settings = (None, settings.1 + 1);
    }
}

#[no_mangle]
pub extern "C" fn nr_column_row_count(column: usize) -> usize {
    with_columns(|columns| {
        columns
            .columns()
            .get(column)
            .map_or(0, |column| column.rows.len())
    })
    .unwrap_or(0)
}

/// Fills `out` with one row. The text is valid until the next call that changes the columns
/// (`nr_column_select`, `nr_columns_refresh`, `nr_drive_start`, `nr_selection_choose`); copy it
/// before then.
///
/// # Safety
/// `out` must point to writable storage for one `NrFileRow`.
#[no_mangle]
pub unsafe extern "C" fn nr_column_row(column: usize, row: usize, out: *mut NrFileRow) -> bool {
    with_columns(|columns| {
        let Some(entry) = columns
            .columns()
            .get(column)
            .and_then(|column| column.rows.get(row))
        else {
            return false;
        };
        let session = running_session(&entry.id);
        // SAFETY: the caller passes storage for one row.
        unsafe { out.write(file_row_with(entry, session.as_ref())) };
        true
    })
    .unwrap_or(false)
}

/// Selects one row: a folder opens as the next column, a file fills the window.
#[no_mangle]
pub extern "C" fn nr_column_select(column: usize, row: usize) {
    with_columns(|columns| {
        if let Err(error) = columns.select(column, row) {
            eprintln!("the selection did not open: {error}");
        }
    });
    watch_open_columns();
}

/// A column's row menu's Open: the terminals selected with the row open together, filling the
/// tab side by side (`Columns::open_terminals_together`); any other row is selected, as a
/// press selects it.
#[no_mangle]
pub extern "C" fn nr_column_open_from_menu(column: usize, row: usize) {
    with_columns(|columns| {
        let opened =
            columns
                .open_terminals_together(column, row)
                .and_then(|together| match together {
                    true => Ok(()),
                    false => columns.select(column, row),
                });
        if let Err(error) = opened {
            eprintln!("the row did not open: {error}");
        }
    });
    watch_open_columns();
}

/// Opens one row as a double click, ⌘↓ or ⌘O does: a folder becomes the next column, a file
/// fills the tab in the columns' place (`Columns::open_row`).
#[no_mangle]
pub extern "C" fn nr_column_open(column: usize, row: usize) {
    with_columns(|columns| {
        if let Err(error) = columns.open_row(column, row) {
            eprintln!("the row did not open: {error}");
        }
    });
    watch_open_columns();
}

/// Whether the open file is a PDF, which the shell draws as its pages with the platform's own
/// PDF renderer (PDFKit on a Mac) in place of the file's details.
pub(crate) fn opened_is_pdf() -> bool {
    with_columns(|columns| {
        let opened = columns.opened()?.clone();
        let path = columns.workspace().path_of(&opened.id).ok()?;
        Some(matches!(opened.kind, EntryKind::File) && crate::workspace::is_pdf_file(&path))
    })
    .flatten()
    .unwrap_or(false)
}

/// Whether the open file is one the platform's own previewer draws (Quick Look on a Mac).
/// Elsewhere there is none, and such a file shows as its details.
pub(crate) fn opened_is_system_preview() -> bool {
    cfg!(target_os = "macos")
        && with_columns(|columns| {
            let opened = columns.opened()?.clone();
            let path = columns.workspace().path_of(&opened.id).ok()?;
            Some(
                matches!(opened.kind, EntryKind::File)
                    && crate::workspace::is_system_preview_file(&path),
            )
        })
        .flatten()
        .unwrap_or(false)
}

/// The path of the open file, or an empty text when none is open.
#[no_mangle]
pub extern "C" fn nr_opened_path() -> NrText {
    lend(
        with_columns(|columns| {
            let id = columns.opened()?.id.clone();
            columns.workspace().path_of(&id).ok()
        })
        .flatten()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default(),
    )
}

/// Shows the folder the shown folder is in (`Columns::open_parent`).
#[no_mangle]
pub extern "C" fn nr_folder_open_parent() {
    with_columns(Columns::open_parent);
    watch_open_columns();
}

/// Keeps the watched folders in step with the open columns and the live HTML pages.
pub(crate) fn watch_open_columns() {
    let Some(mut folders) = with_columns(Columns::directories) else {
        return;
    };
    for folder in crate::page::watch::folders() {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    if let Some(watch) = app().watcher.relock().as_mut() {
        watch.watch(&folders);
    }
}

/// Asks to be told when a folder an open column lists changes on disk. `changed` is called on
/// a thread of the core's own; the shell hops to its UI thread and calls `nr_columns_refresh`.
#[no_mangle]
pub extern "C" fn nr_columns_watch(changed: extern "C" fn()) {
    *app().changed.relock() = Some(changed);
    match crate::drive::watch::FolderWatch::start(move || changed()) {
        Ok(watch) => {
            *app().watcher.relock() = Some(watch);
            watch_open_columns();
        }
        Err(error) => eprintln!("the drive is not being watched: {error}"),
    }
}

/// A shell's reader of the platform's file index. It writes what the index knows about the
/// file at the NUL-terminated `path` into `out` (at most `capacity` bytes of UTF-8), one
/// `key<TAB>value` per line, and returns how many bytes it wrote. The keys are `added` and
/// `opened` (seconds since 1970), `width`, `height`, `duration` (seconds), `camera`,
/// `author`, `comment` and `source`; `author` and `source` may repeat.
/// Calling requires a live NUL-terminated path and an output buffer of `capacity` bytes.
pub type NrFileIndex =
    unsafe extern "C" fn(path: *const c_char, out: *mut u8, capacity: usize) -> usize;

/// Moves `path` to the system trash. Returns -1 when refused, else how many UTF-8 bytes of the
/// item's new path it wrote into `landed` (0 when the platform cannot say).
pub type NrTrash = extern "C" fn(path: *const c_char, landed: *mut u8, capacity: usize) -> i64;

/// Room for the trash to report where it put a file: Windows' longest path, 32,767 UTF-16
/// units, at up to 3 UTF-8 bytes each.
const TRASH_LANDED_CAPACITY: usize = 3 * 32_767;

/// The platform's file readers and trash, passed with the drive to `nr_drive_start` so the
/// first listing already has what the readers say and no folder is listed again because a
/// reader came late. A piece the platform lacks is `None`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrFileSystem {
    /// The platform's file index. Without it the columns and details that come from the index
    /// stay unknown.
    pub file_index: Option<NrFileIndex>,
    /// The platform's name for a file's kind ("PNG image"), as Finder's Kind column says it,
    /// written as `NrFileIndex` writes its lines, or nothing when the platform has none.
    /// Without it the kinds are the drive's own words.
    pub describe_kind: Option<NrFileIndex>,
    /// The system's trash. Without it nothing can be deleted.
    pub trash: Option<NrTrash>,
}

impl NrFileSystem {
    /// Nothing at all: what a drive has before `nr_drive_start`.
    pub const NONE: Self = Self {
        file_index: None,
        describe_kind: None,
        trash: None,
    };
}

/// Gives `workspace` the platform's readers and its trash (for a check, the check's own trash),
/// as far as the shell has provided them.
fn equip(workspace: &mut Workspace) {
    let readers = *app().files.relock();
    if let Some(describe) = readers.describe_kind {
        workspace.set_kind_describer(platform_words(describe));
    }
    if let Some(index) = readers.file_index {
        workspace.set_file_index(platform_file_index(index));
    }
    if let Some(trash) = run_trash(readers.trash) {
        workspace.set_trash(trash);
    }
}

/// What the platform says of a file in a few words, as `describe` writes them: its kind's name
/// or its default app's.
fn platform_words(describe: NrFileIndex) -> crate::workspace::FileWords {
    Box::new(move |path| {
        let text = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut buffer = vec![0u8; 512];
        // SAFETY: the path and writable buffer remain live for this call.
        let written =
            unsafe { describe(text.as_ptr(), buffer.as_mut_ptr(), buffer.len()) }.min(buffer.len());
        (written > 0).then(|| String::from_utf8_lossy(&buffer[..written]).into_owned())
    })
}

/// What the platform's file index knows about a file, as `index` writes it.
fn platform_file_index(index: NrFileIndex) -> crate::workspace::FileIndex {
    Box::new(move |path| {
        let text = std::ffi::CString::new(path.to_string_lossy().as_bytes()).ok()?;
        let mut buffer = vec![0u8; 16 * 1024];
        // SAFETY: the path and writable buffer remain live for this call.
        let written =
            unsafe { index(text.as_ptr(), buffer.as_mut_ptr(), buffer.len()) }.min(buffer.len());
        (written > 0)
            .then(|| crate::workspace::Indexed::parse(&String::from_utf8_lossy(&buffer[..written])))
    })
}

/// The trash the drive deletes into, for the Changes tab's reverts.
pub(crate) fn drive_trash() -> Option<crate::workspace::Trash> {
    run_trash(app().files.relock().trash)
}

/// The trash this run deletes into: a check's own folder, none for any other check, else the
/// system's trash `system` when the shell gave one.
fn run_trash(system: Option<NrTrash>) -> Option<crate::workspace::Trash> {
    // A check names a folder of its own to stand for the trash (`NR_TRASH`): nothing it
    // deletes reaches the system's.
    if let Some(folder) = std::env::var_os("NR_TRASH").filter(|folder| !folder.is_empty()) {
        return Some(crate::workspace::trash_into(PathBuf::from(folder)));
    }
    // Any other check reaches no trash at all: nothing it deletes goes anywhere.
    if crate::launch::get().unattended {
        return None;
    }
    system.map(system_trash)
}

/// The system's trash, moved into through `trash`, which says where it put the file when the
/// platform can.
fn system_trash(trash: NrTrash) -> crate::workspace::Trash {
    Box::new(move |path| {
        let text = std::ffi::CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| std::io::Error::other("the path holds a NUL"))?;
        let mut landed = vec![0u8; TRASH_LANDED_CAPACITY];
        let written = trash(text.as_ptr(), landed.as_mut_ptr(), landed.len());
        let Ok(written) = usize::try_from(written) else {
            return Err(std::io::Error::other("the system refused"));
        };
        // A platform that cannot say, or says more than it was given room for, leaves it unknown.
        Ok((1..=landed.len())
            .contains(&written)
            .then(|| PathBuf::from(String::from_utf8_lossy(&landed[..written]).into_owned())))
    })
}

/// Opens the drive at `directory`, with what the app keeps for it in `metadata`, given the
/// platform's readers before its first folder is listed.
fn open_drive(directory: &Path, metadata: &Path) -> crate::workspace::DriveResult<Columns> {
    let mut workspace = Workspace::open(directory, metadata)?;
    equip(&mut workspace);
    Columns::open(workspace)
}

/// Opens the drive at `directory`, keeping what the app keeps for it under `support`.
fn open_selected(support: &Path, directory: &Path) -> bool {
    let metadata = crate::drive::selection::metadata_directory(support, directory);
    match open_drive(directory, &metadata) {
        Ok(columns) => {
            *app().columns.relock() = Some(columns);
            *app().support.relock() = Some((support.to_path_buf(), directory.to_path_buf()));
            watch_open_columns();
            open_waiting_path();
            true
        }
        Err(error) => {
            eprintln!("the drive did not open: {error}");
            false
        }
    }
}

/// How the drive opened at launch.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrDriveStart {
    /// No drive has been chosen.
    Unchosen,
    Open,
    /// The chosen drive cannot be opened.
    Unavailable,
    /// A check's folder could not be opened, which ends the app (the reason is logged and said
    /// on the error output).
    CheckFailed,
}

/// Opens the drive at launch: a check's own (`NR_WORKSPACE`), else the folder chosen on this
/// computer. `files` is what the platform reads of each file and its
/// trash; the drive has them from its first listing on, and so does a drive chosen later.
#[no_mangle]
pub extern "C" fn nr_drive_start(files: NrFileSystem) -> NrDriveStart {
    *app().files.relock() = files;
    let paths = crate::paths::get();
    let Some(folder) = &paths.fixed_drive else {
        return open_chosen(&paths.support);
    };
    match open_drive(folder, &paths.check_metadata) {
        Ok(columns) => {
            *app().columns.relock() = Some(columns);
            NrDriveStart::Open
        }
        Err(error) => {
            eprintln!("the drive did not open: {error}");
            let folder = folder.to_string_lossy();
            crate::diagnostics::write_with(
                crate::diagnostics::Level::Error,
                "the drive folder could not be opened",
                &serde_json::json!({ "folder": folder }),
            );
            eprintln!("the drive folder could not be opened");
            NrDriveStart::CheckFailed
        }
    }
}

/// Opens the folder chosen on this device, as the settings file keeps it.
fn open_chosen(support: &Path) -> NrDriveStart {
    use crate::drive::selection::Selection;
    match crate::drive::selection::read(&crate::paths::get().settings_file) {
        Selection::Unconfigured => NrDriveStart::Unchosen,
        Selection::Ready(directory) if open_selected(support, &directory) => NrDriveStart::Open,
        Selection::Ready(_) | Selection::Unavailable(_) => NrDriveStart::Unavailable,
    }
}

/// Makes the folder at `directory` the drive and opens it; every running terminal ends,
/// since each ran in the drive that is closing. Returns why not, or an empty text.
///
/// # Safety
/// Both arguments must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_selection_choose(
    support: *const c_char,
    directory: *const c_char,
) -> NrText {
    // SAFETY: the caller passes NUL-terminated strings.
    let (support, directory) = unsafe {
        (
            PathBuf::from(text_of(support)),
            PathBuf::from(text_of(directory)),
        )
    };
    let chosen = match crate::drive::selection::choose(
        &crate::paths::get().settings_file,
        &directory,
        interface_language(),
    ) {
        Ok(chosen) => chosen,
        Err(refusal) => return lend(refusal),
    };
    nr_terminals_quit();
    *app().page.relock() = None;
    *app().page_file.relock() = None;
    if open_selected(&support, &chosen) {
        // The window redraws: a new drive's revision could equal the old one's.
        let mut settings = app().settings.relock();
        settings.1 += 1_000_000;
        lend(String::new())
    } else {
        lend(
            crate::strings::text(
                interface_language(),
                crate::strings::key::ONBOARDING_CHOOSE_REFUSED,
            )
            .to_owned(),
        )
    }
}

/// A path asked for before the drive opened, as when the request launched the app.
static WAITING_PATH: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Opens `path` in the drive in its own tab, as `Columns::open_from_outside` does: what another
/// program or an agent asks the app to open (`open -a Northroom <path>` on a Mac,
/// `northroom <path>` elsewhere). Before the drive is open the path waits for it. Returns why not, or an empty text; the window draws
/// again either way.
///
/// # Safety
/// `path` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_open_path(path: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let path = PathBuf::from(unsafe { text_of(path) });
    if app().columns.relock().is_none() {
        *WAITING_PATH.relock() = Some(path);
        return lend(String::new());
    }
    let refusal = file_operation(|columns| columns.open_from_outside(&path));
    crate::notify_changed();
    refusal
}

/// Opens the path that waited for the drive, now that it is open.
fn open_waiting_path() {
    let Some(path) = WAITING_PATH.relock().take() else {
        return;
    };
    let _ = file_operation(|columns| columns.open_from_outside(&path));
}

/// Runs a file operation and reports a refusal. Returns the refusal's words, or an empty text
/// when it went through; valid until the next file operation.
pub(crate) fn file_operation(
    run: impl FnOnce(&mut Columns) -> crate::workspace::DriveResult<()>,
) -> NrText {
    static REFUSAL: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let outcome = with_columns(run);
    watch_open_columns();
    let refusal = match outcome {
        Some(Err(error)) => error.say(interface_language()),
        _ => String::new(),
    };
    NrText::lend_into(&REFUSAL, refusal)
}

/// Renames a row's file. Returns why not, or an empty text.
///
/// # Safety
/// `title` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_column_rename(
    column: usize,
    row: usize,
    title: *const c_char,
) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let title = unsafe { text_of(title) };
    file_operation(|columns| columns.rename(column, row, &title))
}

/// Whether a running terminal's row takes files dropped on it, by its session.
pub(crate) fn takes_references_now() -> impl Fn(&crate::workspace::Entry) -> bool {
    let running = running_sessions();
    move |entry| file_row(entry, &running).takes_references
}

/// Where a drag of the drive's files `ids` (one per line), from the column that lists them,
/// lands over row `row` of column `column` (-1: its empty space).
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_column_drop_place(
    ids: *const c_char,
    column: usize,
    row: i64,
) -> crate::window::ffi::NrDropPlace {
    use crate::columns::ColumnDrop;
    use crate::window::ffi::NrDropPlace;
    // SAFETY: the caller passes a NUL-terminated string.
    let ids = ids_of(&unsafe { text_of(ids) });
    let takes = takes_references_now();
    let place = with_columns(|columns| {
        columns.column_drop_place(&ids, column, usize::try_from(row).ok(), takes)
    });
    match place {
        Some(ColumnDrop::Folder(_)) => NrDropPlace::Folder,
        Some(ColumnDrop::Terminal(_)) => NrDropPlace::Terminal,
        Some(ColumnDrop::Column) => NrDropPlace::Shown,
        Some(ColumnDrop::Nowhere) | None => NrDropPlace::Nowhere,
    }
}

/// Drops the drive's files `ids` (one per line) where `nr_column_drop_place` puts them: into
/// a folder's row or the column's own folder. A terminal's row takes nothing here: it is
/// handed them by `nr_folder_drop`. Returns why not, or an empty text.
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_column_drop(ids: *const c_char, column: usize, row: i64) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let ids = ids_of(&unsafe { text_of(ids) });
    let takes = takes_references_now();
    file_operation(|columns| columns.column_drop(&ids, column, usize::try_from(row).ok(), takes))
}

/// The column whose folder is the one shown: the last one. The list and the board show that
/// folder alone.
#[no_mangle]
pub extern "C" fn nr_shown_column() -> usize {
    with_columns(|columns| columns.columns().len().saturating_sub(1)).unwrap_or(0)
}

/// Copies files and folders into a column's folder (`Columns::import`): the open panel's
/// choice, or files dropped from another app. `paths` holds one absolute path per line. The
/// copies are one change to undo.
///
/// # Safety
/// `paths` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_folder_import(column: usize, paths: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let sources: Vec<PathBuf> = ids_of(&unsafe { text_of(paths) })
        .into_iter()
        .map(PathBuf::from)
        .collect();
    file_operation(|columns| columns.import(column, &sources))
}

/// Renames the file with `id` in a column's folder.
///
/// # Safety
/// `id` and `title` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_folder_rename(
    column: usize,
    id: *const c_char,
    title: *const c_char,
) -> NrText {
    // SAFETY: the caller passes NUL-terminated strings.
    let (id, title) = unsafe { (text_of(id), text_of(title)) };
    file_operation(|columns| columns.rename_id(column, &id, &title))
}

/// How many bytes the file with `id` holds on disk; 0 when it has gone.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_entry_size(id: *const c_char) -> u64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    with_columns(|columns| columns.workspace().path_of(&id).ok())
        .flatten()
        .and_then(|path| std::fs::metadata(path).ok())
        .map_or(0, |stat| stat.len())
}

/// Where the file with `id` is on disk, for a shell to read its picture or hand it to a
/// drag. Empty when the file has gone.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_entry_path(id: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    lend(
        with_columns(|columns| columns.workspace().path_of(&id).ok())
            .flatten()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// The PNG of the still of the 3D model whose id is `id` (`model_still::picture`): the picture
/// its row shows. Empty for any other file, and while the still is being drawn; asking starts it.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_model_still(id: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    let model = with_columns(|columns| {
        let workspace = columns.workspace();
        // Check the name first so only a 3D model's row pays for a look at the file on disk.
        let named_model = workspace
            .listed_path(&id)
            .and_then(Path::extension)
            .and_then(|extension| extension.to_str())
            .and_then(|extension| {
                crate::workspace::media_of_extension(&extension.to_ascii_lowercase())
            })
            == Some(crate::workspace::Media::Model);
        named_model.then(|| workspace.path_of(&id).ok()).flatten()
    })
    .flatten()
    .filter(|path| crate::workspace::media_of_file(path) == Some(crate::workspace::Media::Model));
    lend(
        model
            .and_then(|path| crate::model_still::picture(&path))
            .map(|file| file.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// What the details pane shows of the file filling the window.
struct OpenedDetails {
    id: String,
    path: String,
    title: String,
    kind: EntryKind,
    /// What stands above its name (`details::shows`).
    shows: crate::preview::details::Shows,
    bytes: u64,
    /// Its kind and size, as the pane says them under its name (`player::summary_line`).
    summary: String,
    /// A picture's or video's size as its file gives it (`details::Shown::media_pixel_size`).
    media_pixel_size: Option<(u32, u32)>,
}

fn opened_details() -> Option<OpenedDetails> {
    with_columns(|columns| {
        let opened = columns.opened()?.clone();
        let path = columns.workspace().path_of(&opened.id).ok()?;
        let stat = std::fs::metadata(&path).ok()?;
        // Only a plain file is read: what it is read as says whether the system draws it.
        let read = (opened.kind == EntryKind::File).then(|| crate::workspace::read_file(&path));
        let shows = crate::preview::details::shows(opened.kind, read.as_ref());
        let details = crate::preview::player::Details {
            // The platform's own words for a file's kind, as the list's Kind column has them.
            kind: (opened.kind != EntryKind::Folder)
                .then(|| columns.workspace().kind_description_at(&path))
                .flatten()
                .unwrap_or_else(|| crate::list::describe_kind(opened.kind).to_owned()),
            // A folder, and a document package listed as a file, have no one size to say.
            size: stat
                .is_file()
                .then(|| crate::list::format_size(Some(stat.len()))),
            ..crate::preview::player::Details::default()
        };
        Some(OpenedDetails {
            id: opened.id.clone(),
            path: path.to_string_lossy().into_owned(),
            title: opened.title.clone(),
            kind: opened.kind,
            shows,
            bytes: stat.len(),
            summary: crate::preview::player::summary_line(&details),
            media_pixel_size: media_pixel_size(shows, &path),
        })
    })
    .flatten()
}

/// What stands above a file's name in the details pane: the core's own type, which is
/// `#[repr(C)]`. A picture is read from `NrDetails::path` no larger than `picture_pixels` on a
/// side; the glyph is `NrDetails::glyph`.
pub use crate::preview::details::Shows as NrDetailsShows;

/// The details pane of the file filling the window (`details.rs`), as last loaded. The
/// preview playing its video or sound takes `file_bytes` as the whole of what it loads. Where
/// everything goes is `details_layout`. Text is valid until the next `nr_details_load`.
#[repr(C)]
pub struct NrDetails {
    pub id: NrText,
    pub path: NrText,
    pub glyph: NrText,
    pub shows: NrDetailsShows,
    pub plays: bool,
    pub picture_pixels: u32,
    pub file_bytes: u64,
}

static DETAILS: std::sync::Mutex<Option<OpenedDetails>> = std::sync::Mutex::new(None);

/// Loads the details pane of the file filling the window into `out`. False when none is open.
///
/// # Safety
/// `out` must point to writable storage for one `NrDetails`.
#[no_mangle]
pub unsafe extern "C" fn nr_details_load(out: *mut NrDetails) -> bool {
    use crate::preview::details::Shows;
    let mut kept = DETAILS.relock();
    *kept = opened_details();
    let Some(details) = kept.as_ref() else {
        return false;
    };
    // SAFETY: the caller passes storage for one value; the text lives in `DETAILS`.
    unsafe {
        out.write(NrDetails {
            id: NrText::borrowing(&details.id),
            path: NrText::borrowing(&details.path),
            glyph: NrText::borrowing(
                crate::list::file_glyph(
                    details.kind,
                    crate::workspace::FileFamily::of_path(std::path::Path::new(&details.path)),
                )
                .name(),
            ),
            shows: details.shows,
            plays: matches!(details.shows, Shows::Video | Shows::Sound),
            picture_pixels: crate::preview::details::picture_pixels(details.shows),
            file_bytes: details.bytes,
        });
    }
    true
}

/// The details pane as last loaded laid out over `width` × `height` (`details::layout`), with
/// what is known of the picture or video (`media`), `fills` when it was opened to fill the tab;
/// none when no details are loaded.
pub(crate) fn details_layout(
    width: f64,
    height: f64,
    media: crate::preview::details::Media,
    fills: bool,
) -> Option<crate::preview::details::Layout> {
    let kept = DETAILS.relock();
    let details = kept.as_ref()?;
    let shown = crate::preview::details::Shown {
        kind: details.kind,
        shows: details.shows,
        title: &details.title,
        summary: &details.summary,
        language: interface_language(),
        media_pixel_size: details.media_pixel_size,
    };
    Some(crate::preview::details::layout(
        width,
        height,
        &shown,
        media,
        fills,
        &mut crate::text::typeset::width,
    ))
}

/// The display size in pixels of the picture or video at `path`, after its orientation, read
/// once per version of the file. It lets the details pane place the media before the picture
/// is decoded or the video plays (`details::layout`). `None` for any other file, or one whose
/// size cannot be read.
fn media_pixel_size(
    shows: crate::preview::details::Shows,
    path: &std::path::Path,
) -> Option<(u32, u32)> {
    use crate::preview::details::Shows;
    match shows {
        Shows::Picture => crate::media::picture_size_once(path),
        Shows::Video => crate::media::probe_once(path).dimensions,
        Shows::Sound | Shows::SystemPicture | Shows::Glyph => None,
    }
}

/// The frame shown at `seconds` of the video at `path`, as a picture no larger than
/// `max_pixels` on either side, turned as the video is shown: a card's or a row's thumbnail
/// (`media::still`), for every kind of video the drive plays. A `CGImage` the caller releases,
/// or null when the file has no video that decodes. It decodes, so call it off the main thread.
///
/// # Safety
/// `path` must point to a NUL-terminated string.
#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn nr_video_still(
    path: *const c_char,
    seconds: f64,
    max_pixels: u32,
) -> *const c_void {
    // SAFETY: the caller passes a NUL-terminated string.
    let path = unsafe { text_of(path) };
    match crate::media::still(&path, seconds, max_pixels) {
        Ok(still) => still.into_raw(),
        Err(error) => {
            eprintln!("no still of {path}: {error}");
            std::ptr::null()
        }
    }
}

/// A video's still off a Mac: sRGB bytes rather than a `CGImage`.
#[cfg(not(target_os = "macos"))]
pub use crate::media::Still as VideoStill;

/// The frame shown at `seconds` of the video at `path`, as a picture no larger than
/// `max_pixels` on either side, turned as the video is shown (`media::still`), as
/// `nr_video_still` makes it on a Mac; `None` when the file has no video that decodes. It
/// decodes, so call it off the main thread.
#[cfg(not(target_os = "macos"))]
pub fn video_still(path: &str, seconds: f64, max_pixels: u32) -> Option<VideoStill> {
    crate::media::still(path, seconds, max_pixels)
        .map_err(|error| eprintln!("no still of {path}: {error}"))
        .ok()
}

/// The picture file at `path` read by the Windows Imaging Component, turned as its orientation
/// says, no larger than `max_pixels` on its long side and in sRGB, its alpha not multiplied in
/// (`media::windows::picture`): what a card, a row or the details pane shows of a picture on
/// Windows, an iPhone's HEIC among them. `None` when WIC cannot decode it. It decodes, so call
/// it off the main thread.
#[cfg(target_os = "windows")]
pub fn picture_thumbnail(path: &str, max_pixels: u32) -> Option<VideoStill> {
    crate::media::picture_thumbnail(path, max_pixels)
}

/// The file just opened that goes to its own application, once, as its path (empty when
/// there is none). The shell opens it.
/// A check never starts an application: with `NR_OPEN_LOG` naming a file the path is written
/// there, and in any other unattended run (`NR_DO`, `NR_SNAPSHOT_DIR`) to the error output;
/// either way the shell is handed nothing.
#[no_mangle]
pub extern "C" fn nr_take_opened_elsewhere() -> NrText {
    let path = take_opened_elsewhere();
    if path.is_empty() {
        return lend(path);
    }
    if let Ok(log) = std::env::var("NR_OPEN_LOG") {
        use std::io::Write;
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .and_then(|mut file| writeln!(file, "{path}"));
        if let Err(error) = written {
            eprintln!("{log} was not written: {error}");
        }
        return lend(String::new());
    }
    if crate::launch::get().unattended {
        eprintln!("opened-elsewhere\t{path}");
        return lend(String::new());
    }
    lend(path)
}

fn take_opened_elsewhere() -> String {
    let path = with_columns(|columns| {
        let path = columns.take_opened_elsewhere()?;
        Some(path.to_string_lossy().into_owned())
    })
    .flatten();
    path.unwrap_or_default()
}

/// Why a column's folder takes no new files; `None` when it does.
/// The folder column `column` shows.
pub(crate) fn column_folder(column: usize) -> Option<PathBuf> {
    with_columns(|columns| {
        let location = columns.columns().get(column)?.location.clone();
        columns.workspace().require_directory(&location).ok()
    })
    .flatten()
}

pub(crate) fn folder_refusal(column: usize) -> Option<&'static str> {
    let protection = with_columns(|columns| {
        let location = columns.columns().get(column)?.location.clone();
        let workspace = columns.workspace();
        let directory = workspace.require_directory(&location).ok()?;
        workspace
            .protection_of(&directory)
            .filter(|protection| protection.protects_contents())
    })
    .flatten();
    protection.map(|protection| crate::strings::text(interface_language(), protection.words()))
}

/// Lists the folder shown as its board shows it and returns how many cards it has.
#[no_mangle]
pub extern "C" fn nr_board_load() -> usize {
    let rows = with_columns(|columns| columns.board_rows()).unwrap_or_default();
    let count = rows.len();
    // What each card shows, from what the drive reads of its file.
    let faces: Vec<crate::board::CardFace> = rows
        .iter()
        .zip(board_reads_of(&rows))
        .map(|(entry, read)| {
            crate::board::card_face(
                entry.kind,
                entry.family == crate::workspace::FileFamily::Code,
                read.as_deref(),
            )
        })
        .collect();
    *BOARD_FACES.relock() = faces;
    *BOARD_PICTURE_SIZES.relock() = board_picture_sizes(&rows);
    *app().board_rows.relock() = rows;
    crate::board::ffi::forget_previews();
    count
}

/// Each card's picture or video size as last loaded, which sets the card's shape
/// (`board::card_shape`). `None` for other files, or when the file does not say.
static BOARD_PICTURE_SIZES: std::sync::Mutex<Vec<Option<(u32, u32)>>> =
    std::sync::Mutex::new(Vec::new());

/// The width and height of card `index`'s picture or video, as last loaded; none for any other
/// card, or a file that does not say.
pub(crate) fn board_picture_size(index: usize) -> Option<(u32, u32)> {
    BOARD_PICTURE_SIZES.relock().get(index).copied().flatten()
}

/// The width and height of each of `rows`' pictures and videos, read from their files on
/// several threads, once per version of each file.
fn board_picture_sizes(rows: &[crate::workspace::Entry]) -> Vec<Option<(u32, u32)>> {
    let paths: Vec<Option<(EntryKind, PathBuf)>> = with_columns(|columns| {
        rows.iter()
            .map(|entry| {
                matches!(entry.kind, EntryKind::Image | EntryKind::Video)
                    .then(|| columns.workspace().path_of(&entry.id).ok())
                    .flatten()
                    .map(|path| (entry.kind, path))
            })
            .collect()
    })
    .unwrap_or_default();
    crate::threads::each_on_threads(&paths, crate::workspace::LISTING_SPREAD, |file| {
        let (kind, path) = file.as_ref()?;
        match kind {
            EntryKind::Video => crate::media::probe_once(path).dimensions,
            _ => crate::media::picture_size_once(path),
        }
    })
}

/// What each card last loaded shows.
static BOARD_FACES: std::sync::Mutex<Vec<crate::board::CardFace>> =
    std::sync::Mutex::new(Vec::new());

/// The files read for the board, by id, with the size and time each was read at: a board
/// reloads often, and a file is read again only when it has changed.
type BoardRead = (
    Option<u64>,
    i64,
    Option<std::sync::Arc<crate::workspace::FileRead>>,
);
static BOARD_READS: std::sync::Mutex<Option<std::collections::HashMap<String, BoardRead>>> =
    std::sync::Mutex::new(None);

/// A file on the board as the drive reads it (`workspace::read_file`).
fn board_read_of(
    entry: &crate::workspace::Entry,
) -> Option<std::sync::Arc<crate::workspace::FileRead>> {
    let mut reads = BOARD_READS.relock();
    let reads = reads.get_or_insert_with(std::collections::HashMap::new);
    if let Some((size, modified, read)) = reads.get(&entry.id) {
        if *size == entry.size_bytes && *modified == entry.modified_seconds {
            return read.clone();
        }
    }
    let path = with_columns(|columns| columns.workspace().path_of(&entry.id).ok()).flatten();
    let read = path.map(|path| std::sync::Arc::new(crate::workspace::read_file(&path)));
    reads.insert(
        entry.id.clone(),
        (entry.size_bytes, entry.modified_seconds, read.clone()),
    );
    read
}

/// What the drive reads of each card's file (`board_read_of`), `None` for a card whose file is
/// not read: the files not read before, or changed since, are read on several threads at once.
fn board_reads_of(
    rows: &[crate::workspace::Entry],
) -> Vec<Option<std::sync::Arc<crate::workspace::FileRead>>> {
    let read_for =
        |entry: &crate::workspace::Entry| matches!(entry.kind, EntryKind::File | EntryKind::Page);
    let mut reads = BOARD_READS.relock();
    let reads = reads.get_or_insert_with(std::collections::HashMap::new);
    let unread: Vec<&crate::workspace::Entry> = rows
        .iter()
        .filter(|entry| {
            read_for(entry)
                && reads.get(&entry.id).is_none_or(|(size, modified, _)| {
                    *size != entry.size_bytes || *modified != entry.modified_seconds
                })
        })
        .collect();
    let paths: Vec<Option<PathBuf>> = with_columns(|columns| {
        unread
            .iter()
            .map(|entry| columns.workspace().path_of(&entry.id).ok())
            .collect()
    })
    .unwrap_or_default();
    let read = crate::threads::each_on_threads(&paths, crate::workspace::LISTING_SPREAD, |path| {
        path.as_deref()
            .map(|path| std::sync::Arc::new(crate::workspace::read_file(path)))
    });
    for (entry, read) in unread.iter().zip(read) {
        reads.insert(
            entry.id.clone(),
            (entry.size_bytes, entry.modified_seconds, read),
        );
    }
    rows.iter()
        .map(|entry| {
            read_for(entry)
                .then(|| reads.get(&entry.id).and_then(|(_, _, read)| read.clone()))
                .flatten()
        })
        .collect()
}

/// The text a board card shows for a plain file. Only a file read as text counts, never a PDF,
/// an HTML page or another file whose bytes happen to be plain. A database shows the top of its
/// first table.
pub(crate) fn board_text_of(entry: &crate::workspace::Entry) -> Option<std::sync::Arc<str>> {
    match board_read_of(entry)?.as_ref() {
        crate::workspace::FileRead::Text(text) | crate::workspace::FileRead::Database(text) => {
            Some(std::sync::Arc::from(text.as_str()))
        }
        _ => None,
    }
}

/// What the card at `index` of the board shows, as last loaded.
pub(crate) fn board_card_face(index: usize) -> crate::board::CardFace {
    BOARD_FACES
        .relock()
        .get(index)
        .copied()
        .unwrap_or(crate::board::CardFace::Own)
}

/// Fills `out` with one card of the board as last loaded. Text is valid until the next
/// `nr_board_load`.
///
/// # Safety
/// `out` must point to writable storage for one `NrFileRow`.
#[no_mangle]
pub unsafe extern "C" fn nr_board_row(index: usize, out: *mut NrFileRow) -> bool {
    let rows = app().board_rows.relock();
    let Some(entry) = rows.get(index) else {
        return false;
    };
    let session = running_session(&entry.id);
    // SAFETY: the caller passes storage for one row.
    unsafe { out.write(file_row_with(entry, session.as_ref())) };
    true
}

/// A rectangle in points from the board's top-left corner, y downwards.
#[repr(C)]
pub struct NrRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The kind of each card last loaded.
fn board_kinds() -> Vec<EntryKind> {
    app()
        .board_rows
        .relock()
        .iter()
        .map(|entry| entry.kind)
        .collect()
}

/// The board's zoom: how many columns it shows when it is wide enough for them.
pub(crate) fn board_zoom() -> usize {
    with_columns(|columns| columns.board_columns()).unwrap_or(crate::board::DEFAULT_COLUMN_COUNT)
}

/// One wheel event over a board `width` points wide. `delta_y` is positive towards the
/// person, as a web page counts it, in `delta_mode` units (0 pixels, 1 lines, 2 pages). With
/// ⌘ or ctrl held it zooms: returns the column count now shown once the wheel has travelled
/// far enough for a step, and 0 until then. -1 is a wheel that does not zoom, which the
/// caller scrolls with.
pub(crate) fn board_zoom_wheel(
    delta_y: f64,
    delta_mode: u32,
    ctrl: bool,
    meta: bool,
    width: f64,
) -> i64 {
    // The travel of the zoom gesture so far that has not yet made a step.
    static PENDING: std::sync::Mutex<f64> = std::sync::Mutex::new(0.0);
    if !crate::board::is_zoom_wheel(ctrl, meta, delta_y) {
        return -1;
    }
    let mut pending = PENDING.relock();
    let pixels = crate::board::wheel_pixel_delta(delta_y, delta_mode);
    with_columns(|columns| {
        let stepped =
            crate::board::zoom_wheel(&mut pending, columns.board_columns(), width, pixels)?;
        columns.set_board_columns(stepped);
        Some(stepped as i64)
    })
    .flatten()
    .unwrap_or(0)
}

/// Lays the cards last loaded out on a board `width` points wide showing up to `zoom`
/// columns (0: the board's own zoom). Returns the height the cards take.
pub(crate) fn board_layout(width: f64, zoom: usize) -> f64 {
    let zoom = if zoom == 0 { board_zoom() } else { zoom };
    let kinds = board_kinds();
    let faces = BOARD_FACES.relock().clone();
    let sizes = BOARD_PICTURE_SIZES.relock().clone();
    let cards: Vec<f64> = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            crate::board::card_shape(
                *kind,
                faces
                    .get(index)
                    .copied()
                    .unwrap_or(crate::board::CardFace::Own),
                sizes.get(index).copied().flatten(),
            )
        })
        .collect();
    let (frames, height) = crate::board::layout_shapes(&cards, width, zoom);
    *app().board_frames.relock() = frames;
    height
}

/// Where card `index` sits, as last laid out.
///
/// # Safety
/// `out` must point to writable storage for one `NrRect`.
#[no_mangle]
pub unsafe extern "C" fn nr_board_frame(index: usize, out: *mut NrRect) -> bool {
    let frames = app().board_frames.relock();
    let Some(frame) = frames.get(index) else {
        return false;
    };
    // SAFETY: the caller passes storage for one rectangle.
    unsafe {
        out.write(NrRect {
            x: frame.x,
            y: frame.y,
            width: frame.width,
            height: frame.height,
        });
    }
    true
}

/// Opens the file with `id` in the folder shown: a folder becomes the folder shown, a file
/// fills the window.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_folder_open(id: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    with_columns(|columns| {
        if let Err(error) = columns.open_in_folder(&id) {
            eprintln!("the file did not open: {error}");
        }
    });
    watch_open_columns();
}

/// Opens the film or video `id` in the folder shown and starts it playing as its view opens:
/// what the play button on its card does.
///
/// # Safety
/// `id` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_folder_play(id: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    with_columns(|columns| {
        if let Err(error) = columns.play_in_folder(&id) {
            eprintln!("the file did not open: {error}");
        }
    });
    watch_open_columns();
}

/// Whether the open file's view starts it playing as it opens (`nr_folder_play`). Asking
/// settles it: a view asks once, as it opens.
#[no_mangle]
pub extern "C" fn nr_opened_takes_play() -> bool {
    with_columns(Columns::take_starts_playing).unwrap_or(false)
}

/// How folders are shown: as columns while no drive is open.
pub(crate) fn folder_view() -> crate::columns::View {
    with_columns(|columns| columns.view()).unwrap_or_default()
}

/// Shows folders as `view`.
pub(crate) fn set_folder_view(view: crate::columns::View) {
    with_columns(|columns| columns.set_view(view));
}

/// One row of the list view. Text is valid until the next `nr_list_load`.
#[repr(C)]
pub struct NrListRow {
    pub title: NrText,
    pub file: NrFileRow,
    /// How many open folders the row is inside.
    pub depth: u32,
    /// Whether the row is a folder whose files are listed under it.
    pub expanded: bool,
    /// The id of the folder the row's file is in; empty for the drive's own folder.
    pub parent: NrText,
    /// Words standing in for an open folder's files; when not empty the row is no file.
    pub placeholder: NrText,
}

/// Lists the folder shown, in the list's order, and returns how many rows it has.
#[no_mangle]
pub extern "C" fn nr_list_load() -> usize {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, crate::drive::file_stat::seconds);
    let rows = with_columns(|columns| {
        let rows = columns.list_rows(now);
        columns.show_list(&rows);
        rows
    })
    .unwrap_or_default();
    let count = rows.len();
    *app().list_rows.relock() = rows;
    count
}

/// Fills `out` with one row of the list as last loaded.
///
/// # Safety
/// `out` must point to writable storage for one `NrListRow`.
#[no_mangle]
pub unsafe extern "C" fn nr_list_row(index: usize, out: *mut NrListRow) -> bool {
    let rows = app().list_rows.relock();
    let Some(row) = rows.get(index) else {
        return false;
    };
    let session = running_session(&row.entry.id);
    // SAFETY: the caller passes storage for one row.
    unsafe {
        out.write(NrListRow {
            title: NrText::borrowing(&row.entry.title),
            file: file_row_with(&row.entry, session.as_ref()),
            depth: row.depth,
            expanded: row.expanded,
            parent: NrText::borrowing(row.parent.folder_id.as_deref().unwrap_or("")),
            placeholder: NrText::borrowing(
                row.placeholder
                    .map_or("", |placeholder| placeholder.words(interface_language())),
            ),
        });
    }
    true
}

/// Opens a row of the list as last loaded: a folder becomes the folder shown, a file fills
/// the window.
pub(crate) fn list_open(index: usize) {
    let id = app()
        .list_rows
        .relock()
        .get(index)
        .map(|row| row.entry.id.clone());
    if let Some(id) = id {
        with_columns(|columns| {
            if let Err(error) = columns.open_in_folder(&id) {
                eprintln!("the file did not open: {error}");
            }
        });
        watch_open_columns();
    }
}

/// Opens the file with `id` that is in the folder with `parent_id` (empty: the drive's own):
/// that folder becomes the folder shown, then the file opens.
///
/// # Safety
/// Both arguments must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_list_open_in(parent_id: *const c_char, id: *const c_char) {
    // SAFETY: the caller passes NUL-terminated strings.
    let (parent, id) = unsafe { (text_of(parent_id), text_of(id)) };
    let location = crate::workspace::Location {
        folder_id: (!parent.is_empty()).then_some(parent),
    };
    with_columns(|columns| {
        if let Err(error) = columns.open_in(&location, &id) {
            eprintln!("the file did not open: {error}");
        }
    });
    watch_open_columns();
}

/// Where the parts of a row sit across it (`list::RowLayout`).
#[repr(C)]
pub struct NrRowLayout {
    pub caret: f64,
    pub icon: f64,
    pub name: f64,
    pub rename: f64,
    pub placeholder: f64,
    /// The row's height, and its pitch.
    pub height: f64,
}

impl From<crate::list::RowLayout> for NrRowLayout {
    fn from(layout: crate::list::RowLayout) -> Self {
        Self {
            caret: layout.caret,
            icon: layout.icon,
            name: layout.name,
            rename: layout.rename,
            placeholder: layout.placeholder,
            height: layout.height,
        }
    }
}

/// A list row `depth` open folders deep, from the left of its Name column.
#[no_mangle]
pub extern "C" fn nr_list_row_layout(depth: u32) -> NrRowLayout {
    crate::list::list_row_layout(depth).into()
}

/// A column's row, from its left.
#[no_mangle]
pub extern "C" fn nr_column_row_layout() -> NrRowLayout {
    crate::list::column_row_layout().into()
}

/// What the list's header says after "Name": how many rows are `selected`, or how many files
/// the list as last loaded lists when none is.
#[no_mangle]
pub extern "C" fn nr_list_count_label(selected: usize) -> NrText {
    let listed = app()
        .list_rows
        .relock()
        .iter()
        .filter(|row| row.placeholder.is_none())
        .count();
    lend(crate::list::count_label(listed, selected))
}

/// The name on the chip that follows the pointer while `count` files are dragged, the first
/// named `first_title`.
///
/// # Safety
/// `first_title` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_drag_label(count: usize, first_title: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let title = unsafe { text_of(first_title) };
    lend(crate::list::drag_label(count, &title))
}

/// Opens the folder with `id` in place in the list, or closes it.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_list_toggle_expanded(id: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    with_columns(|columns| columns.toggle_expanded(&id));
}

/// The list's sort: `key` is a column's number (`list::Column::number`), or -1 for the
/// folder's own order; `ascending` says which way.
#[repr(C)]
pub struct NrListSort {
    pub key: i32,
    pub ascending: bool,
}

#[no_mangle]
pub extern "C" fn nr_list_sort() -> NrListSort {
    with_columns(|columns| columns.list_sort())
        .flatten()
        .map_or(
            NrListSort {
                key: -1,
                ascending: true,
            },
            |sort| NrListSort {
                key: sort.key.number() as i32,
                ascending: sort.ascending,
            },
        )
}

/// A click on a list header: a new column sorts ascending, the same one flips direction.
pub(crate) fn list_toggle_sort(key: u32) {
    if let Some(column) = crate::list::Column::numbered(key) {
        with_columns(|columns| columns.toggle_list_sort(column));
    }
}

/// How many columns the list shows, Name first.
#[no_mangle]
pub extern "C" fn nr_list_column_count() -> usize {
    with_columns(|columns| columns.list_columns().len()).unwrap_or(0)
}

/// Shows a column at the list's right end, or takes it away. Name stays.
pub(crate) fn list_toggle_column(key: u32) {
    if let Some(column) = crate::list::Column::numbered(key) {
        with_columns(|columns| columns.toggle_list_column(column));
    }
}

/// Moves a shown column to place `to` among the columns (1 is the first after Name).
pub(crate) fn list_move_column(key: u32, to: usize) {
    if let Some(column) = crate::list::Column::numbered(key) {
        with_columns(|columns| columns.move_list_column(column, to));
    }
}

/// The command palette while it is open (`commands::Palette`).
static PALETTE: std::sync::Mutex<Option<crate::commands::Palette>> = std::sync::Mutex::new(None);

/// The commands the palette offers: the drive's while a drive is open; none in Settings.
fn palette_commands() -> Vec<crate::commands::Command> {
    if nr_settings_tab() >= 0 || with_columns(|_| ()).is_none() {
        Vec::new()
    } else {
        crate::commands::drive_commands(interface_language())
    }
}

fn with_palette<T: Default>(action: impl FnOnce(&mut crate::commands::Palette) -> T) -> T {
    PALETTE.relock().as_mut().map(action).unwrap_or_default()
}

/// Opens the command palette (⌘K): every command offered, the first lit.
#[no_mangle]
pub extern "C" fn nr_palette_open() {
    *PALETTE.relock() = Some(crate::commands::Palette::open(&palette_commands()));
}

#[no_mangle]
pub extern "C" fn nr_palette_close() {
    *PALETTE.relock() = None;
}

/// The palette's rows become the commands that match `query`, best first; the best is lit.
///
/// # Safety
/// `query` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_palette_search(query: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let query = unsafe { text_of(query) };
    with_palette(|palette| palette.search(&palette_commands(), &query));
}

/// Lays the palette out over a window `width` × `height` points (`Palette::layout`) and
/// returns where its parts sit (`commands::Parts`). `None` while it is closed.
pub fn palette_layout(width: f64, height: f64) -> Option<crate::commands::Parts> {
    with_palette(|palette| {
        palette.layout(width, height);
        Some(palette.parts(interface_language(), &mut crate::text::typeset::width))
    })
}

/// What a key did in the palette.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPaletteKey {
    /// The palette does not take it.
    PassOn,
    /// It is drawn again.
    Redraw,
    /// It closes and the command `run` names runs.
    Run,
    Close,
}

/// A key in the palette: ↓, ↑, Return and Escape are its own. `run` is valid while the
/// commands are.
///
/// # Safety
/// `run` must point to writable storage for one `NrText`.
#[no_mangle]
pub unsafe extern "C" fn nr_palette_key(press: NrKeyPress, run: *mut NrText) -> NrPaletteKey {
    use crate::commands::{PaletteAnswer, PaletteKey};
    let key = match press.key {
        NrKey::Down => PaletteKey::Down,
        NrKey::Up => PaletteKey::Up,
        NrKey::Return => PaletteKey::Return,
        NrKey::Escape => PaletteKey::Escape,
        _ => return NrPaletteKey::PassOn,
    };
    match with_palette(|palette| Some(palette.key(key))) {
        Some(PaletteAnswer::Redraw) => NrPaletteKey::Redraw,
        Some(PaletteAnswer::Run(id)) => {
            // SAFETY: the caller passes storage for one text.
            unsafe { run.write(NrText::borrowing(id)) };
            NrPaletteKey::Run
        }
        Some(PaletteAnswer::Close) => NrPaletteKey::Close,
        None => NrPaletteKey::PassOn,
    }
}

/// Lights palette row `row`, which the pointer is over. Returns whether the lit row changed.
#[no_mangle]
pub extern "C" fn nr_palette_point(row: usize) -> bool {
    with_palette(|palette| palette.point(row))
}

/// What the palette's field shows while nothing is typed.
#[no_mangle]
pub extern "C" fn nr_palette_placeholder() -> NrText {
    NrText::borrowing(crate::commands::placeholder(interface_language()))
}

/// Shows the files whose names start with a dot, or hides them again.
pub(crate) fn toggle_hidden_files() {
    if let Some(Err(error)) = with_columns(Columns::toggle_hidden) {
        eprintln!("the folder did not show its hidden files: {error}");
    }
    watch_open_columns();
}

/// Sets a column's width in points, within its limits; 0 gives it back its own.
pub(crate) fn list_set_width(key: u32, width: f64) {
    if let Some(column) = crate::list::Column::numbered(key) {
        with_columns(|columns| columns.set_list_width(column, width));
    }
}

/// What row `row` says in shown column `column` (1 is the first after Name), as last loaded.
#[no_mangle]
pub extern "C" fn nr_list_cell(row: usize, column: usize) -> NrText {
    let rows = app().list_rows.relock();
    let cell = rows
        .get(row)
        .and_then(|listed| listed.cells.get(column.wrapping_sub(1)))
        .cloned()
        .unwrap_or_default();
    drop(rows);
    lend(cell)
}

#[no_mangle]
pub extern "C" fn nr_tabs_count() -> usize {
    with_columns(|columns| columns.tab_count()).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn nr_tabs_active() -> usize {
    with_columns(|columns| columns.active_tab()).unwrap_or(0)
}

/// The name a tab shows: the file or folder it holds, "Changes" for a Changes tab, or for a tab
/// filled by one terminal, the task its session reports. Valid until the next call to this
/// function.
#[no_mangle]
pub extern "C" fn nr_tab_title(index: usize) -> NrText {
    static TITLE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let (title, terminal) = with_columns(|columns| {
        if columns.tab_changes(index).is_some() {
            let words =
                crate::strings::text(interface_language(), crate::strings::key::CHANGES_TITLE);
            return (words.to_owned(), None);
        }
        let terminals = columns.tab_terminal_ids(index);
        let filled = columns.tab_kind(index) == 5 && terminals.len() == 1;
        (
            columns.tab_title(index),
            filled.then(|| terminals[0].clone()),
        )
    })
    .unwrap_or_default();
    // The columns are released before the sessions are read.
    let title = terminal
        .and_then(|id| crate::sessions::ffi::tab_task(&id, &title))
        .unwrap_or(title);
    NrText::lend_into(&TITLE, title)
}

/// Opens a new tab beside the active one and makes it active.
#[no_mangle]
pub extern "C" fn nr_tab_open() {
    with_columns(|columns| {
        if let Err(error) = columns.open_tab() {
            eprintln!("the tab did not open: {error}");
        }
    });
    watch_open_columns();
}

#[no_mangle]
pub extern "C" fn nr_tab_select(index: usize) {
    with_columns(|columns| columns.select_tab(index));
}

/// Whether the tab at `index` can close: any but the last one open.
#[no_mangle]
pub extern "C" fn nr_tab_closable(index: usize) -> bool {
    with_columns(|columns| columns.can_close_tab(index)).unwrap_or(false)
}

/// The name of the glyph before a tab's title (`glyph::Glyph`), chosen by what the tab holds.
#[no_mangle]
pub extern "C" fn nr_tab_glyph(index: usize) -> NrText {
    NrText::borrowing(
        with_columns(|columns| columns.tab_glyph(index))
            .unwrap_or(crate::list::Glyph::House)
            .name(),
    )
}

/// What a tab filled by a terminal shows in place of its glyph: the mark of the agent
/// started in it, by name among the app's pictures (empty with none, which keeps the terminal
/// glyph, and for a tab holding anything else), and what its session is doing, as a row's
/// `terminal::activity` number (0 for none running).
#[repr(C)]
pub struct NrTabTerminal {
    pub logo: NrText,
    pub activity: u32,
}

#[no_mangle]
pub extern "C" fn nr_tab_terminal(index: usize) -> NrTabTerminal {
    let terminal =
        with_columns(|columns| columns.tab_terminal(index).map(|opened| opened.id.clone()))
            .flatten();
    let session = terminal.and_then(|id| running_session(&id));
    NrTabTerminal {
        logo: NrText::borrowing(session.as_ref().map_or("", RunningSession::logo)),
        activity: session.map_or(0, |session| session.activity),
    }
}

/// Whether a closed tab is kept to reopen.
#[no_mangle]
pub extern "C" fn nr_tab_reopenable() -> bool {
    with_columns(|columns| columns.can_reopen_closed_tab()).unwrap_or(false)
}

/// Brings back the tab closed last (`Columns::reopen_closed_tab`) and makes it active.
#[no_mangle]
pub extern "C" fn nr_tab_reopen_closed() {
    with_columns(|columns| {
        if let Err(error) = columns.reopen_closed_tab() {
            eprintln!("the closed tab did not reopen: {error}");
        }
    });
    crate::changes::ffi::tab_reopened();
    watch_open_columns();
}

/// Closes a tab. The last one stays open. A terminal in it that runs nothing but its shell,
/// and that no other tab shows, ends with it (`session_at_rest`); an agent or a program at
/// work runs on and stays in the sessions list. A Changes tab's root is no longer watched for
/// it.
#[no_mangle]
pub extern "C" fn nr_tab_close(index: usize) {
    let (terminals, changes) = with_columns(|columns| {
        if !columns.can_close_tab(index) {
            return (Vec::new(), None);
        }
        let terminals = columns.tab_terminal_ids(index);
        let changes = columns.tab_changes(index).map(Path::to_path_buf);
        columns.close_tab(index);
        let ended = terminals
            .into_iter()
            .filter(|id| !columns.shows_terminal(id))
            .collect();
        (ended, changes)
    })
    .unwrap_or_default();
    if let Some(root) = changes {
        crate::changes::ffi::tab_closed(&root);
    }
    for id in terminals {
        if session_at_rest(&id) {
            end_session(&id);
        }
    }
    watch_open_columns();
}

#[no_mangle]
pub extern "C" fn nr_history_can_go_back() -> bool {
    nr_settings_tab() >= 0 || with_columns(|columns| columns.can_go_back()).unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn nr_history_can_go_forward() -> bool {
    with_columns(|columns| columns.can_go_forward()).unwrap_or(false)
}

/// Back: closes an open file, else the folder shown before, else the enclosing folder.
#[no_mangle]
pub extern "C" fn nr_history_back() {
    // Back from settings returns to the folder.
    if nr_settings_tab() >= 0 {
        nr_settings_close();
        return;
    }
    with_columns(|columns| {
        if let Err(error) = columns.go_back() {
            eprintln!("could not go back: {error}");
        }
    });
    watch_open_columns();
}

#[no_mangle]
pub extern "C" fn nr_history_forward() {
    with_columns(|columns| {
        if let Err(error) = columns.go_forward() {
            eprintln!("could not go forward: {error}");
        }
    });
    watch_open_columns();
}

/// Lists every open column again, after the folder changed on disk.
#[no_mangle]
pub extern "C" fn nr_columns_refresh() {
    with_columns(|columns| {
        if let Err(error) = columns.refresh() {
            eprintln!("the drive did not refresh: {error}");
        }
    });
}

/// Fills `out` with the file filling the window. Returns false when none is open. The text is
/// valid as `nr_column_row`'s is.
///
/// # Safety
/// `out` must point to writable storage for one `NrOpened`.
#[no_mangle]
pub unsafe extern "C" fn nr_opened(out: *mut NrOpened) -> bool {
    with_columns(|columns| {
        let Some(opened) = columns.opened() else {
            return false;
        };
        // SAFETY: the caller passes storage for one value.
        unsafe {
            out.write(NrOpened {
                title: NrText::borrowing(&opened.title),
                kind: file_kind(opened.kind),
            });
        }
        true
    })
    .unwrap_or(false)
}

/// The largest Markdown file the window reads to draw a page.
pub(crate) const PAGE_MAX_BYTES: u64 = 1024 * 1024;

/// Reads the page filling the window from its file. Returns how many blocks it has, or -1
/// when no page is open or its file cannot be read. Text from `nr_page_*` is valid until the
/// next `nr_page_load`.
#[no_mangle]
pub extern "C" fn nr_page_load() -> i64 {
    let opened = with_columns(opened_as_page).flatten();
    let mut page = app().page.relock();
    let mut file = app().page_file.relock();
    match opened {
        Some(PageRead::Editable { id, title, path }) => match read_page_file(&path) {
            Some(markdown) => keep_page(&mut page, &mut file, id, &title, markdown),
            None => {
                *page = None;
                *file = None;
            }
        },
        Some(PageRead::ProtectedPage { title, path }) => {
            let shown = crate::workspace::read_text_file(&path).map(|text| {
                crate::page::Page::from_file(
                    &title,
                    &text,
                    interface_language(),
                    &mut crate::page::create_block_id,
                )
            });
            show_read_only(&mut page, &mut file, shown);
        }
        Some(PageRead::PlainText { title, path }) => {
            let shown = crate::workspace::read_text_file(&path).map(|text| {
                crate::board::plain_text_page(
                    crate::workspace::name_without_extension(&title),
                    &text,
                )
            });
            show_read_only(&mut page, &mut file, shown);
        }
        None => {
            *page = None;
            *file = None;
        }
    }
    page.as_ref().map_or(-1, |page| page.blocks.len() as i64)
}

/// How the file filling the window reads as a page.
enum PageRead {
    /// A page file, kept with its text so that edits reach it (`edit_page`).
    Editable {
        id: String,
        title: String,
        path: PathBuf,
    },
    /// A page whose file the system protects, or that a link leads to from outside the drive:
    /// it reads as a page that cannot be typed in.
    ProtectedPage { title: String, path: PathBuf },
    /// A plain-text file, read as a page that cannot be typed in.
    PlainText { title: String, path: PathBuf },
}

/// The file filling the window as a page, or none when it is not one.
fn opened_as_page(columns: &mut Columns) -> Option<PageRead> {
    let opened = columns.opened()?.clone();
    let path = columns.workspace().path_of(&opened.id).ok()?;
    match opened.kind {
        EntryKind::Page if columns.workspace().protection_of(&path).is_some() => {
            Some(PageRead::ProtectedPage {
                title: opened.title,
                path,
            })
        }
        EntryKind::Page => Some(PageRead::Editable {
            id: opened.id,
            title: opened.title,
            path,
        }),
        EntryKind::File
            if crate::workspace::text_form(&opened.title)
                == crate::workspace::TextForm::PlainPage =>
        {
            Some(PageRead::PlainText {
                title: opened.title,
                path,
            })
        }
        _ => None,
    }
}

/// The Markdown of the page file at `path`, when it is no larger than `PAGE_MAX_BYTES` and
/// reads.
fn read_page_file(path: &Path) -> Option<String> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > PAGE_MAX_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Shows the page read from the file `id` named `title`, its text `markdown`. The page is kept
/// while its file still says what was last read or written, so blocks keep their ids across the
/// page's own saves.
fn keep_page(
    page: &mut Option<crate::page::Page>,
    file: &mut Option<(String, String)>,
    id: String,
    title: &str,
    markdown: String,
) {
    let unchanged = page.is_some()
        && file
            .as_ref()
            .is_some_and(|(open, text)| *open == id && *text == markdown);
    if unchanged {
        return;
    }
    *page = Some(crate::page::Page::from_file(
        title,
        &markdown,
        interface_language(),
        &mut crate::page::create_block_id,
    ));
    *file = Some((id, markdown));
    // The file changed under the recorded steps.
    app().page_history.relock().clear();
}

/// Shows `shown`, a page that cannot be typed in. No file is kept for it, so no edit reaches it
/// (`edit_page`).
fn show_read_only(
    page: &mut Option<crate::page::Page>,
    file: &mut Option<(String, String)>,
    shown: Option<crate::page::Page>,
) {
    *page = shown;
    *file = None;
    app().page_history.relock().clear();
}

/// Whether the open page cannot be typed in: a plain-text file read as a page.
pub(crate) fn page_read_only() -> bool {
    let shown = app().page.relock().is_some();
    shown && app().page_file.relock().is_none()
}

/// How the open file is shown when it is a text file that is not a page file; `None` when it
/// is not one (no file is open, it is another kind, or it is not read as text: too large, a
/// NUL byte, not UTF-8).
pub(crate) fn opened_text_form() -> Option<crate::workspace::TextForm> {
    with_opened_text(|form, _| form)
}

/// The open delimited text file (`TextForm::Table`) or Excel workbook (`opened_workbook_path`)
/// as a table, or the open database (`opened_database_path`) as its tables. A workbook or
/// database that cannot be read is `Table::Unreadable`; `None` when no such file is open. Its
/// view is laid out from this (`window::ffi::table_view`).
pub fn opened_table() -> Option<crate::sheets::layout::Table> {
    let table = if let Some(path) = opened_database_path() {
        Some(crate::sheets::database::table(&path))
    } else if let Some(path) = opened_workbook_path() {
        Some(read_workbook(&path))
    } else {
        opened_text().and_then(|(form, text)| {
            if form != crate::workspace::TextForm::Table {
                return None;
            }
            let name = with_columns(|columns| columns.opened().map(|opened| opened.title.clone()))
                .flatten()?;
            Some(crate::sheets::layout::Table::Sheets(
                crate::sheets::spreadsheet::read(&name, &text),
            ))
        })
    };
    OPENED_TABLE.relock().clone_from(&table);
    table
}

/// The sheet last chosen, by the id of the file it was chosen in.
static TABLE_SHEET: std::sync::Mutex<Option<(String, usize)>> = std::sync::Mutex::new(None);

fn opened_id() -> Option<String> {
    with_columns(|columns| columns.opened().map(|opened| opened.id.clone())).flatten()
}

/// The sheet of the open table to show: the one last chosen in it while the file still has
/// it, else the first. Delimited text is one sheet.
#[no_mangle]
pub extern "C" fn nr_table_sheet() -> usize {
    use crate::sheets::layout::Table;
    let chosen = TABLE_SHEET.relock().clone();
    let sheets = |table: Table| match table {
        Table::Sheets(workbook) | Table::Database(workbook) => workbook.sheets.len(),
        Table::Unreadable(_) => 0,
    };
    let count = match (opened_database_path(), opened_workbook_path()) {
        (Some(path), _) => sheets(crate::sheets::database::table(&path)),
        (None, Some(path)) => sheets(read_workbook(&path)),
        (None, None) => 1,
    };
    match (chosen, opened_id()) {
        (Some((id, sheet)), Some(opened)) if id == opened && sheet < count => sheet,
        _ => 0,
    }
}

/// Shows sheet `sheet` of the open table, as a press on its tab does.
#[no_mangle]
pub extern "C" fn nr_table_choose_sheet(sheet: usize) {
    *TABLE_SHEET.relock() = opened_id().map(|id| (id, sheet));
}

/// The open file's path when it is a 3D model shown as a still (a `.glb`, by its name); empty
/// otherwise.
#[no_mangle]
pub extern "C" fn nr_opened_model_path() -> NrText {
    let path = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        let model = std::path::Path::new(&opened.title)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"));
        if opened.kind != EntryKind::File || !model {
            return None;
        }
        columns.workspace().path_of(&opened.id).ok()
    })
    .flatten();
    lend(
        path.map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default(),
    )
}

/// The open file's path when it is an HTML page that runs as a page (`FileRead::Html`: an
/// HTML file outside a film's `scenes` folder); empty otherwise. The shell runs it in the
/// system's web view with its own folder served and the network closed.
pub(crate) fn opened_html_path() -> String {
    let path = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        if opened.kind != EntryKind::File {
            return None;
        }
        columns.workspace().path_of(&opened.id).ok()
    })
    .flatten()
    .filter(|path| {
        matches!(
            crate::workspace::read_file(path),
            crate::workspace::FileRead::Html(_)
        )
    });
    path.map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The open file's path when it is an Excel workbook shown as a table: an `.xlsx` or `.xlsm`
/// of at most 16 MB (a larger one opens in its own application).
pub(crate) fn opened_workbook_path() -> Option<std::path::PathBuf> {
    let path = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        if opened.kind != EntryKind::File
            || !crate::sheets::workbook::is_workbook_name(&opened.title)
        {
            return None;
        }
        columns.workspace().path_of(&opened.id).ok()
    })
    .flatten()?;
    let stat = std::fs::metadata(&path).ok()?;
    (stat.is_file() && stat.len() <= crate::sheets::workbook::FILE_MAX_BYTES).then_some(path)
}

/// The open file's path when it is a SQLite database (`database::is_database`), whatever its
/// name: its view shows its tables.
pub(crate) fn opened_database_path() -> Option<std::path::PathBuf> {
    let path = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        if opened.kind != EntryKind::File {
            return None;
        }
        columns.workspace().path_of(&opened.id).ok()
    })
    .flatten()?;
    crate::sheets::database::is_database(&path).then_some(path)
}

/// A workbook read: its path, size and time, and the table it is.
type WorkbookRead = (
    std::path::PathBuf,
    u64,
    Option<std::time::SystemTime>,
    crate::sheets::layout::Table,
);

/// The workbook at `path` as a table, read once per version of the file (its size and time):
/// the view asks again on every change of the window.
fn read_workbook(path: &std::path::Path) -> crate::sheets::layout::Table {
    static READ: std::sync::Mutex<Option<WorkbookRead>> = std::sync::Mutex::new(None);
    let stat = std::fs::metadata(path).ok();
    let version = (
        stat.as_ref().map_or(0, |stat| stat.len()),
        stat.and_then(|stat| stat.modified().ok()),
    );
    let mut read = READ.relock();
    if let Some((known, size, modified, table)) = read.as_ref() {
        if known == path && (*size, *modified) == version {
            return table.clone();
        }
    }
    let table = match crate::sheets::workbook::read(path) {
        Ok(workbook) => crate::sheets::layout::Table::Sheets(workbook),
        Err(failure) => {
            eprintln!("the workbook was not read: {}", failure.0);
            crate::sheets::layout::Table::Unreadable(crate::sheets::layout::Held::Workbook)
        }
    };
    *read = Some((path.to_path_buf(), version.0, version.1, table.clone()));
    table
}

/// The open table as `opened_table` last read it, for its view's layout (`sheets::layout`).
static OPENED_TABLE: std::sync::Mutex<Option<crate::sheets::layout::Table>> =
    std::sync::Mutex::new(None);

pub(crate) fn with_opened_table<T>(
    action: impl FnOnce(&crate::sheets::layout::Table) -> T,
) -> Option<T> {
    OPENED_TABLE.relock().as_ref().map(action)
}

/// The open text file's text (`opened_text_form` is some), valid until the next call that
/// lends text; empty otherwise.
#[no_mangle]
pub extern "C" fn nr_opened_text() -> NrText {
    lend(opened_text().map(|(_, text)| text).unwrap_or_default())
}

/// The source highlighter (`highlight::SCRIPT`), for a JavaScript engine to evaluate once
/// before it evaluates any `nr_opened_syntax_call`. Static.
#[no_mangle]
pub extern "C" fn nr_syntax_script() -> NrText {
    NrText::borrowing(crate::highlight::SCRIPT)
}

/// The JavaScript expression that colours `code`, the open file's text as the shell shows it:
/// a promise of the answer for `nr_syntax_answer`, settled once the highlighter's grammars
/// have compiled (in the same evaluation when they have). Empty when the file stays plain (no
/// grammar for its kind, too long, no file open). Valid until the next call that lends text.
///
/// # Safety
/// `code` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_opened_syntax_call(code: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let code = unsafe { text_of(code) };
    let title =
        with_columns(|columns| columns.opened().map(|opened| opened.title.clone())).flatten();
    lend(
        title
            .and_then(|title| crate::highlight::call(&code, crate::list::name_extension(&title)))
            .unwrap_or_default(),
    )
}

/// As `nr_opened_syntax_call`, for `code` from any file with `extension` (`list::name_extension`,
/// as `nr_changes_diff_load` gives it): the expression that colours it, or empty when it stays
/// plain. Valid until the next call that lends text.
///
/// # Safety
/// `extension` and `code` must be NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_syntax_call(extension: *const c_char, code: *const c_char) -> NrText {
    // SAFETY: the caller passes NUL-terminated strings.
    let (extension, code) = unsafe { (text_of(extension), text_of(code)) };
    lend(crate::highlight::call(&code, &extension).unwrap_or_default())
}

/// A stretch of coloured source (`highlight::Run`). `colored` false: the text is the shell's
/// foreground and `color` is not read.
#[repr(C)]
pub struct NrSyntaxRun {
    pub text: NrText,
    pub colored: bool,
    pub color: NrColor,
    pub italic: bool,
    pub bold: bool,
    pub underline: bool,
    pub strikethrough: bool,
}

/// The coloured source the highlighter answered.
fn syntax_runs() -> std::sync::MutexGuard<'static, Vec<crate::highlight::Run>> {
    static RUNS: std::sync::Mutex<Vec<crate::highlight::Run>> = std::sync::Mutex::new(Vec::new());
    RUNS.relock()
}

/// Reads `answer`, what a promise `nr_opened_syntax_call` made settled with, into runs for
/// `nr_syntax_run` in place of the last answer's. Returns how many; 0 when it is not the
/// highlighter's answer or the text is empty, and the text stays plain.
///
/// # Safety
/// `answer` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_syntax_answer(answer: *const c_char) -> usize {
    // SAFETY: the caller passes a NUL-terminated string.
    let answer = unsafe { text_of(answer) };
    let mut runs = syntax_runs();
    *runs = crate::highlight::runs(&answer).unwrap_or_default();
    runs.len()
}

/// Fills `out` with run `index` of the last answer. Returns false past the last. The text is
/// valid until the next `nr_syntax_answer`.
///
/// # Safety
/// `out` must point to writable storage for one `NrSyntaxRun`.
#[no_mangle]
pub unsafe extern "C" fn nr_syntax_run(index: usize, out: *mut NrSyntaxRun) -> bool {
    let runs = syntax_runs();
    let Some(run) = runs.get(index) else {
        return false;
    };
    let [red, green, blue, alpha] = run
        .color
        .unwrap_or_default()
        .map(|channel| f32::from(channel) / 255.0);
    // SAFETY: the caller passes storage for one run.
    unsafe {
        out.write(NrSyntaxRun {
            text: NrText::borrowing(&run.text),
            colored: run.color.is_some(),
            color: NrColor {
                red,
                green,
                blue,
                alpha,
            },
            italic: run.italic,
            bold: run.bold,
            underline: run.underline,
            strikethrough: run.strikethrough,
        });
    }
    true
}

fn opened_text() -> Option<(crate::workspace::TextForm, String)> {
    with_opened_text(|form, text| (form, text.to_owned()))
}

/// Identifies a version of the open text file: its id and title (the title chooses how it is
/// shown), its path, and its size and last change on disk.
#[derive(PartialEq)]
struct TextStamp {
    id: String,
    title: String,
    path: PathBuf,
    bytes: u64,
    /// None where the system keeps no time of change: such a file is read every time.
    modified: Option<std::time::SystemTime>,
}

/// The open text file as last read, with its stamp; `None` inside when it is not read as
/// text (too large, a NUL byte, not UTF-8).
type ReadText = (TextStamp, Option<(crate::workspace::TextForm, String)>);

/// The open text file as last read: read again only when its stamp changes, not at every
/// change of the columns.
static OPENED_TEXT: std::sync::Mutex<Option<ReadText>> = std::sync::Mutex::new(None);

/// `action` with how the open text file is shown and its text, when a text file that is not
/// a page file is open (`opened_text_form`).
fn with_opened_text<T>(action: impl FnOnce(crate::workspace::TextForm, &str) -> T) -> Option<T> {
    let (id, title, path) = with_columns(|columns| {
        let opened = columns.opened()?.clone();
        if opened.kind != EntryKind::File {
            return None;
        }
        let path = columns.workspace().path_of(&opened.id).ok()?;
        Some((opened.id, opened.title, path))
    })
    .flatten()?;
    if crate::workspace::is_pdf_file(&path) {
        return None;
    }
    // A file that cannot be looked at cannot be read either (`workspace::read_file`).
    let stat = std::fs::metadata(&path).ok()?;
    let stamp = TextStamp {
        id,
        title,
        path,
        bytes: stat.len(),
        modified: stat.modified().ok(),
    };
    let mut kept = OPENED_TEXT.relock();
    let unchanged = stamp.modified.is_some() && kept.as_ref().is_some_and(|(was, _)| *was == stamp);
    if !unchanged {
        let read = crate::workspace::read_text_file(&stamp.path)
            .map(|text| (crate::workspace::text_form(&stamp.title), text));
        *kept = Some((stamp, read));
    }
    let (_, read) = kept.as_ref()?;
    read.as_ref().map(|(form, text)| action(*form, text))
}

/// Runs an edit on the open page and, when it changed anything, writes the page to its file.
pub(crate) fn edit_page<T>(edit: impl FnOnce(&mut crate::page::Page) -> (bool, T)) -> Option<T> {
    let app = app();
    let (changed, result, text, id) = {
        let mut page = app.page.relock();
        let page = page.as_mut()?;
        // A page with no file of its own (a plain-text file read as a page) takes no edit.
        let id = app.page_file.relock().as_ref()?.0.clone();
        // Every change is an undo step; a call that changed nothing leaves none.
        let mut history = app.page_history.relock();
        history.record(page);
        let (changed, result) = edit(page);
        if !changed {
            history.forget_last();
        }
        (changed, result, page.to_file(), id)
    };
    if changed {
        match with_columns(|columns| columns.workspace().write_text(&id, &text)) {
            Some(Ok(())) => {
                *app.page_file.relock() = Some((id, text));
            }
            Some(Err(error)) => eprintln!("the page was not saved: {error}"),
            None => {}
        }
    }
    Some(result)
}

fn utf16_of(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Where the caret goes for a press on the line being typed in that lands on the character at
/// `offset`: after the image it lands on, or -1 when it lands on no image.
///
/// # Safety
/// `text` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_inline_press(text: *const c_char, offset: usize) -> i64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let raw = utf16_of(&unsafe { text_of(text) });
    crate::page::inline_edit::press_on_image(&raw, offset).map_or(-1, |caret| caret as i64)
}

/// What the image whose `![` is at `start` in a line of the open page shows while the line is
/// typed in, as the page shows it at rest (`page::layout`). `kind` is 0 for a picture (returns
/// the file's id; `nr_page_picture_size` sizes it), 1 for a video or sound (returns its alt
/// text, set in the line's own type), 2 for no such media file (returns its alt text, or its
/// address when it has none, set small and quiet).
///
/// # Safety
/// `text` must point to a NUL-terminated string; `kind` to a writable `NrImageShows`.
#[no_mangle]
pub unsafe extern "C" fn nr_inline_image(
    text: *const c_char,
    start: usize,
    kind: *mut NrImageShows,
) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let raw = utf16_of(&unsafe { text_of(text) });
    let image = crate::page::inline_edit::image_spans(&raw)
        .into_iter()
        .find(|&(from, _)| from == start)
        .and_then(|(from, until)| {
            crate::page::inline::parse(&String::from_utf16_lossy(&raw[from..until]))
                .into_iter()
                .find_map(|segment| segment.image)
        });
    let shows = image.map_or(
        crate::page::media::ImageShows::Missing(String::new()),
        |image| open_page_image_shows(&image),
    );
    // SAFETY: the caller passes a writable u32.
    unsafe { lend_image_shows(shows, kind) }
}

// MARK: The page's editor (`page::editor`)

/// The open page as the editor reads and changes it: every change an undo step, saved.
struct OpenPage;

impl crate::page::editor::Pages for OpenPage {
    fn read<T>(&self, read: impl FnOnce(&crate::page::Page) -> T) -> Option<T> {
        app().page.relock().as_ref().map(read)
    }

    fn edit<T>(&mut self, edit: impl FnOnce(&mut crate::page::Page) -> (bool, T)) -> Option<T> {
        edit_page(edit)
    }

    fn rename(&mut self, title: &str) -> bool {
        set_page_title(title)
    }

    fn undo(&mut self, redo: bool) -> bool {
        undo_page(redo)
    }

    fn read_only(&self) -> bool {
        page_read_only()
    }
}

static PAGE_EDITOR: std::sync::Mutex<crate::page::editor::Editor> =
    std::sync::Mutex::new(crate::page::editor::Editor::new());

fn with_page_editor<T>(
    run: impl FnOnce(&mut crate::page::editor::Editor, &mut OpenPage) -> T,
) -> T {
    let mut editor = PAGE_EDITOR.relock();
    run(&mut editor, &mut OpenPage)
}

/// A field as the boundary numbers it: 0 none, 1 the title, 2 a block's words, 3 a table's
/// cell (`row`, `column`), 4 a media block's caption.
fn field_of(kind: u32, block: i64, row: i64, column: i64) -> Option<crate::page::editor::Field> {
    use crate::page::editor::Field;
    let at = |value: i64| usize::try_from(value).ok();
    match kind {
        1 => Some(Field::Title),
        2 => Some(Field::Block(at(block)?)),
        3 => Some(Field::Cell {
            block: at(block)?,
            row: at(row)?,
            column: at(column)?,
        }),
        4 => Some(Field::Caption(at(block)?)),
        _ => None,
    }
}

/// The editor as the shell draws it.
#[repr(C)]
pub struct NrPageEditor {
    /// The blocks selected whole, -1 for none.
    pub chosen_first: i64,
    pub chosen_last: i64,
    /// The media block that has the keys (-1 none), and whether it shows its ring.
    pub media: i64,
    pub media_ring: bool,
    /// The block whose gutter shows (-1 none), and the button under the pointer: 0 none,
    /// 1 the `+`, 2 the handle.
    pub hovered: i64,
    pub button: u32,
    /// Where dragged blocks would land (-1 nowhere): before the block, or after it.
    pub drop_block: i64,
    pub drop_after: bool,
    /// What is typed in, numbered as `field_of` numbers it; `block` is -1 for the title, and
    /// `row` and `column` -1 but in a cell.
    pub field: u32,
    pub block: i64,
    pub row: i64,
    pub column: i64,
    /// The field holds a block's formatted words (`inline_style` shows them).
    pub formatted: bool,
}

/// The editor as it stands.
#[no_mangle]
pub extern "C" fn nr_page_editor() -> NrPageEditor {
    use crate::page::editor::{Button, Field};
    with_page_editor(|editor, pages| {
        let index = |value: Option<usize>| value.map_or(-1, |value| value as i64);
        let chosen = editor.chosen_range();
        let (field, block, row, column) = match editor.editing {
            None => (0, -1, -1, -1),
            Some(Field::Title) => (1, -1, -1, -1),
            Some(Field::Block(block)) => (2, block as i64, -1, -1),
            Some(Field::Cell { block, row, column }) => {
                (3, block as i64, row as i64, column as i64)
            }
            Some(Field::Caption(block)) => (4, block as i64, -1, -1),
        };
        NrPageEditor {
            chosen_first: index(chosen.map(|(first, _, _)| first)),
            chosen_last: index(chosen.map(|(_, last, _)| last)),
            media: index(editor.media.map(|(block, _)| block)),
            media_ring: editor.media.is_some_and(|(_, ring)| ring),
            hovered: index(editor.hovered),
            button: match editor.button {
                Button::None => 0,
                Button::Plus => 1,
                Button::Handle => 2,
            },
            drop_block: index(editor.drop.map(|(block, _)| block)),
            drop_after: editor.drop.is_some_and(|(_, after)| after),
            field,
            block,
            row,
            column,
            formatted: editor.formatted(pages),
        }
    })
}

/// A new page view: nothing typed in, selected or pointed at.
#[no_mangle]
pub extern "C" fn nr_page_editor_reset() {
    with_page_editor(|editor, _| *editor = crate::page::editor::Editor::new());
}

/// What is under a press, from the core's hit test of the page as laid out (`nr_page_hit`),
/// its slot named as a field: -1 is none for every block; `button` 0 none, 1 the `+`, 2 the
/// handle of block `row`; `slot` the slot of words there, numbered as `NrPageEditor.field`;
/// `below` the press is below the page's last words.
#[repr(C)]
pub struct NrPagePress {
    pub row: i64,
    pub button: u32,
    pub block: i64,
    pub media: i64,
    pub todo: i64,
    pub slot: u32,
    pub slot_block: i64,
    pub slot_row: i64,
    pub slot_column: i64,
    pub below: bool,
}

fn button_of(button: u32) -> crate::page::editor::Button {
    use crate::page::editor::Button;
    match button {
        1 => Button::Plus,
        2 => Button::Handle,
        _ => Button::None,
    }
}

/// A press on the page (`page::editor::Editor::press`), ⇧ held or not, with the field as the
/// shell holds it.
///
/// # Safety
/// `text` must point to a NUL-terminated string.
pub fn page_press(
    hit: NrPagePress,
    shift: bool,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    let at = |value: i64| usize::try_from(value).ok();
    let hit = crate::page::editor::Hit {
        row: at(hit.row),
        button: button_of(hit.button),
        block: at(hit.block),
        media: at(hit.media),
        todo: at(hit.todo),
        slot: field_of(hit.slot, hit.slot_block, hit.slot_row, hit.slot_column),
        below: hit.below,
    };
    with_page_editor(|editor, pages| editor.press(pages, hit, shift, typing))
}

/// The pointer is over block `row`'s row (-1 none) and gutter `button`. Returns whether what
/// shows changed.
#[no_mangle]
pub extern "C" fn nr_page_hover(row: i64, button: u32) -> bool {
    with_page_editor(|editor, pages| {
        editor.hover(pages, usize::try_from(row).ok(), button_of(button))
    })
}

/// A handle's press has travelled (`dx`, `dy`) points on a screen of `scale` pixels a point,
/// the pointer at `y` of the page as `view` last laid it out (`board::ffi::page_drop_at` says
/// where the blocks dragged would land). `taken`: it is a drag.
pub fn page_handle_move(
    view: usize,
    y: f64,
    dx: f64,
    dy: f64,
    scale: f64,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    let drop_at = |first: usize, last: usize| crate::board::ffi::page_drop_at(view, y, first, last);
    with_page_editor(|editor, pages| editor.handle_move(pages, dx, dy, scale, drop_at, typing))
}

/// A handle's press ends, ⇧ held or not. `taken`: it was a drag.
pub fn page_handle_up(
    shift: bool,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.handle_up(pages, shift, typing))
}

/// A key the page has while no field is typed in (`page::keys::Key::from_press`).
pub fn page_key(press: NrKeyPress) -> crate::page::editor::Answer {
    use crate::page::keys::{held as held_of, Key};
    with_page_editor(|editor, pages| editor.key(pages, Key::from_press(press), held_of(press.held)))
}

/// A shortcut while the page has the keys, before the menu bar. `taken`: the page took it.
pub fn page_shortcut(press: NrKeyPress) -> crate::page::editor::Answer {
    use crate::page::keys::{held as held_of, Key};
    with_page_editor(|editor, pages| {
        editor.shortcut(pages, Key::from_press(press), held_of(press.held))
    })
}

/// A shortcut while a field has the keys. `taken`: the page took it.
pub fn page_field_shortcut(
    press: NrKeyPress,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    use crate::page::keys::{held as held_of, Key};
    with_page_editor(|editor, pages| {
        editor.field_shortcut(pages, Key::from_press(press), held_of(press.held), typing)
    })
}

/// A command of the field's text view, named as the key that asks for it
/// (`page::keys::Command::from_press`). `taken`: the page took it, and the field does nothing
/// more with it.
pub fn page_text_command(
    command: NrKeyPress,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| {
        editor.command(
            pages,
            crate::page::keys::Command::from_press(command),
            typing,
        )
    })
}

/// The field moved its caret a line up (`up`) or down from `from`, as `Effect::Vertical` asked,
/// and holds the selection `start..end` now. `taken`: the caret moved.
pub fn page_vertical(
    from: usize,
    up: bool,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, _| editor.vertical(from, up, typing))
}

/// `replacement` is about to go over `start..end` of the field, held as `typing`; `marked`: an
/// input method is composing. `taken`: the page made the change itself, and the field does
/// not.
pub fn page_typed(
    start: usize,
    end: usize,
    replacement: &str,
    marked: bool,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.typed(pages, start, end, replacement, marked, typing))
}

/// The field open in the page, if any.
pub(crate) fn page_open_field() -> Option<crate::page::editor::Field> {
    with_page_editor(|editor, _| editor.editing)
}

/// The field's text changed by itself.
pub fn page_changed(typing: &crate::page::editor::Typing) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.changed(pages, typing))
}

/// The field's selection changed.
pub fn page_selection(typing: &crate::page::editor::Typing) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.selection(pages, typing))
}

/// Typing ends (the field let go of the keys), keeping what was typed.
pub fn page_end_typing(typing: &crate::page::editor::Typing) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.end_typing(pages, typing))
}

/// The `/` menu while it is open: the block it hangs under and its layout
/// (`menus::slash::Layout`). `None` while it is closed.
pub fn page_slash() -> Option<(usize, crate::menus::slash::Layout)> {
    let language = interface_language();
    with_page_editor(|editor, _| {
        let slash = editor.slash.as_ref()?;
        let entries: Vec<crate::menus::slash::Entry> = slash
            .shown
            .iter()
            .map(|&command| {
                let entry = &crate::page::edit::COMMANDS[command];
                crate::menus::slash::Entry {
                    label: crate::strings::text(language, entry.label),
                    shortcut: entry.shortcut,
                    icon: entry.icon,
                    command,
                }
            })
            .collect();
        let layout = crate::menus::slash::layout(
            crate::page::edit::commands_heading(language),
            &entries,
            slash.lit,
            slash.offset,
            &mut crate::text::typeset::width,
        );
        Some((slash.block, layout))
    })
}

/// The wheel moved over the `/` menu (`nr_page_slash_scroll`).
pub(crate) fn scroll_page_slash(delta: f64, unit: u32) {
    with_page_editor(|editor, _| editor.scroll_slash(delta, unit));
}

/// The pointer is over `/` menu entry `lit`.
#[no_mangle]
pub extern "C" fn nr_page_slash_light(lit: usize) {
    with_page_editor(|editor, _| editor.light_slash(lit));
}

/// `/` menu entry `command` was chosen with the pointer.
pub fn page_slash_choose(
    command: usize,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.choose_slash(pages, command, typing))
}

/// The clipboard's text, pasted after the selected blocks (`Effect::Paste`).
pub fn page_paste_blocks(markdown: &str) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.paste_blocks(pages, markdown))
}

/// A right click on cell `row`, `column` of table `block`: typing ends, and the cell's menu
/// (`menu_page_table`) is about that cell.
pub fn page_table_menu_open(
    block: usize,
    row: usize,
    column: usize,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.open_table_menu(pages, block, row, column, typing))
}

/// The open page's block `block` for its menu (`menus::page_block_menu`).
pub(crate) fn page_block_kind(block: usize) -> Option<crate::page::BlockKind> {
    app()
        .page
        .relock()
        .as_ref()?
        .blocks
        .get(block)
        .map(|found| found.kind.clone())
}

/// How many columns table `block` of the open page has.
pub(crate) fn page_table_columns(block: usize) -> usize {
    match page_block_kind(block) {
        Some(crate::page::BlockKind::Table { rows, .. }) => rows.first().map_or(0, Vec::len),
        _ => 0,
    }
}

/// What choosing an entry of a page's menu left for the shell (`page_menu_answer`).
static PAGE_MENU_ANSWER: std::sync::Mutex<Option<crate::page::editor::Answer>> =
    std::sync::Mutex::new(None);

/// An entry of the block menu or the table cell menu was chosen (`menus::ffi::nr_menu_run`).
fn run_page_menu(
    run: impl FnOnce(&mut crate::page::editor::Editor, &mut OpenPage) -> crate::page::editor::Answer,
) {
    let answer = with_page_editor(run);
    *PAGE_MENU_ANSWER.relock() = Some(answer);
}

pub(crate) fn run_page_block_menu(command: crate::menus::PageBlockCommand) {
    run_page_menu(|editor, pages| editor.block_menu(pages, command));
}

pub(crate) fn run_page_table_menu(command: crate::menus::PageTableCommand) {
    run_page_menu(|editor, pages| editor.table_menu(pages, command));
}

/// What the page's menu entry last chosen left for the shell, as `page_press` answers; nothing
/// taken and nothing to do when none was chosen.
pub fn page_menu_answer() -> crate::page::editor::Answer {
    PAGE_MENU_ANSWER.relock().take().unwrap_or_default()
}

/// For a check: opens the menu of block `row`, or presses its `+` (`plus`).
pub fn page_gutter(
    row: usize,
    plus: bool,
    typing: &crate::page::editor::Typing,
) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.gutter(pages, row, plus, typing))
}

/// For a check: media block `row`'s keys and caption field (`Editor::debug_media`).
pub fn page_debug_media(row: usize, step: &str) -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.debug_media(pages, row, step))
}

/// For a check: typing starts at the end of the first block.
pub fn page_edit_first() -> crate::page::editor::Answer {
    with_page_editor(|editor, pages| editor.edit_first(pages))
}

/// Whether this run traces the line typed in (`NR_PAGE_TRACE`, `launch::Launch::trace_page`).
#[no_mangle]
pub extern "C" fn nr_page_tracing() -> bool {
    crate::launch::get().trace_page
}

/// How a block's raw `text` shows while it is typed in, in type of `size` and `weight` on a
/// line `width` wide with the selection `start..end` in UTF-16 offsets
/// (`page::inline_edit::style`): its spans, each a stretch of UTF-16 offsets with how it shows,
/// and its images, each from its `!` to its `)`.
pub fn inline_style(
    text: &str,
    start: usize,
    end: usize,
    size: f64,
    weight: f64,
    width: f64,
) -> crate::page::inline_edit::Style {
    crate::page::inline_edit::style(&utf16_of(text), (start, end), size, weight, width)
}

/// Steps the open page back one change, or forward again, and saves it. Returns whether
/// there was a step to take.
fn undo_page(redo: bool) -> bool {
    let app = app();
    let (text, id) = {
        let mut page = app.page.relock();
        let Some(page) = page.as_mut() else {
            return false;
        };
        let mut history = app.page_history.relock();
        let stepped = if redo {
            history.redo(page)
        } else {
            history.undo(page)
        };
        if !stepped {
            return false;
        }
        let Some(id) = app.page_file.relock().as_ref().map(|file| file.0.clone()) else {
            return false;
        };
        (page.to_file(), id)
    };
    match with_columns(|columns| columns.workspace().write_text(&id, &text)) {
        Some(Ok(())) => *app.page_file.relock() = Some((id, text)),
        Some(Err(error)) => eprintln!("the page was not saved: {error}"),
        None => {}
    }
    true
}

/// Gives the open page a new title. A page whose file states its title keeps the file's
/// name; any other page is its file's name, so the file is renamed. Returns false when the
/// drive refused the new name.
fn set_page_title(title: &str) -> bool {
    let title = title.trim().to_owned();
    if title.is_empty() {
        return true;
    }
    let stated = app().page.relock().as_ref().map(|page| page.states_title);
    match stated {
        Some(true) => {
            edit_page(|page| {
                let changed = page.title != title;
                page.title.clone_from(&title);
                (changed, ())
            });
            true
        }
        Some(false) => {
            let Some(id) = app().page_file.relock().as_ref().map(|file| file.0.clone()) else {
                return true;
            };
            let refusal = file_operation(|columns| {
                let column = columns.columns().len().saturating_sub(1);
                columns.rename_id(column, &id, &title)
            });
            if refusal.length > 0 {
                return false;
            }
            if let Some(page) = app().page.relock().as_mut() {
                page.title = title;
            }
            true
        }
        None => true,
    }
}

/// What an image of the open page shows, beside the page's file.
pub(crate) fn open_page_image_shows(
    image: &crate::page::inline::Image,
) -> crate::page::media::ImageShows {
    use crate::page::media::ImageShows;
    let missing = || {
        ImageShows::Missing(if image.alt.is_empty() {
            image.src.clone()
        } else {
            image.alt.clone()
        })
    };
    let Some(id) = app().page_file.relock().as_ref().map(|file| file.0.clone()) else {
        return missing();
    };
    with_columns(|columns| {
        let workspace = columns.workspace();
        let path = workspace.path_of(&id).ok()?;
        Some(crate::page::media::image_shows(workspace, &path, image))
    })
    .flatten()
    .unwrap_or_else(missing)
}

/// What an image written in a page shows (`page::media::ImageShows`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrImageShows {
    /// The picture whose id is lent.
    Picture,
    /// Its words, lent.
    Words,
    /// The words that say it is missing, lent.
    Missing,
}

/// Writes what an image shows as `kind` and lends its id or words.
///
/// # Safety
/// `kind` must point to a writable `NrImageShows`.
unsafe fn lend_image_shows(
    shows: crate::page::media::ImageShows,
    kind: *mut NrImageShows,
) -> NrText {
    use crate::page::media::ImageShows;
    let (code, text) = match shows {
        ImageShows::Picture { id, .. } => (NrImageShows::Picture, id),
        ImageShows::Words(words) => (NrImageShows::Words, words),
        ImageShows::Missing(words) => (NrImageShows::Missing, words),
    };
    // SAFETY: the caller passes a writable value.
    unsafe { kind.write(code) };
    lend(text)
}

/// What player block `index` of the open page shows and plays (`page::media::player_shows`),
/// and of the file it plays its stored size and length; with the block's id. `None` for a
/// block that is no player.
fn page_player_shown(index: usize) -> Option<(String, crate::page::players::Shown)> {
    let (block, html) = {
        let page = app().page.relock();
        let page = page.as_ref()?;
        (
            page.blocks.get(index)?.id.clone(),
            page.player_html(index)?.to_owned(),
        )
    };
    let page_id = app().page_file.relock().as_ref().map(|file| file.0.clone());
    let tag = crate::page::media::player_tag(&html)?;
    // A page with no file of its own plays nothing: it has no folder to name files from.
    let (shows, played) = with_columns(|columns| {
        let workspace = columns.workspace();
        let page_path = page_id
            .as_ref()
            .and_then(|id| workspace.path_of(id).ok())
            .unwrap_or_default();
        let shows = crate::page::media::player_shows(workspace, &page_path, &tag);
        let played = shows
            .plays
            .as_ref()
            .and_then(|(_, file)| workspace.path_of(file).ok());
        (shows, played)
    })?;
    let probed = played
        .map(|path| crate::media::probe_once(&path))
        .unwrap_or_default();
    let (media_width, media_height) = probed.dimensions.unwrap_or((0, 0));
    let shown = crate::page::players::Shown {
        shows,
        media_width: f64::from(media_width),
        media_height: f64::from(media_height),
        duration: probed
            .duration_seconds
            .filter(|seconds| seconds.is_finite())
            .unwrap_or(0.0),
    };
    Some((block, shown))
}

fn page_players() -> std::sync::MutexGuard<'static, crate::page::players::PagePlayers> {
    app().page_players.relock()
}

/// The texts `nr_page_player_lay` hands out: the block's id, its words, its file's id and its
/// poster's.
static PAGE_PLAYER_LENT: std::sync::Mutex<[String; 4]> =
    std::sync::Mutex::new([String::new(), String::new(), String::new(), String::new()]);

/// What a page's player block shows (`page::players::Kind`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPlayerKind {
    /// No player.
    None,
    /// Its words.
    Words,
    /// A picture.
    Video,
    /// A sound's pill.
    Sound,
}

/// A player block of the open page as laid out (`page::players::PagePlayers::lay`).
#[repr(C)]
pub struct NrPagePlayer {
    pub kind: NrPlayerKind,
    pub block: NrText,
    /// The element's box (its words' line), and the picture's place in it.
    pub frame: NrRect,
    pub picture: NrRect,
    /// It draws the preview's controls.
    pub controls: bool,
    /// What it shows changed, so its controls are made again. `slept`: it went from awake to
    /// rest and its file closed (the shell releases the sound output and the layer).
    pub changed: bool,
    pub slept: bool,
    pub words: NrText,
    /// The ids of its file and of its poster's picture; empty when it has none.
    pub file: NrText,
    pub poster: NrText,
}

fn nr_rect(rect: crate::preview::player::video_controls::Rect) -> NrRect {
    NrRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// Lays out player block `index` of the open page with its box's top at `top`, in a column
/// from `side`, `column` wide. The texts are valid until the next call of this.
#[no_mangle]
pub extern "C" fn nr_page_player_lay(
    index: usize,
    top: f64,
    side: f64,
    column: f64,
) -> NrPagePlayer {
    use crate::page::players::Kind;
    let zero = || NrRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    };
    let mut lent = PAGE_PLAYER_LENT.relock();
    let Some((block, shown)) = page_player_shown(index) else {
        *lent = Default::default();
        let none = || NrText::borrowing("");
        return NrPagePlayer {
            kind: NrPlayerKind::None,
            block: none(),
            frame: zero(),
            picture: zero(),
            controls: false,
            changed: false,
            slept: false,
            words: none(),
            file: none(),
            poster: none(),
        };
    };
    *lent = [
        block.clone(),
        shown.shows.words.clone(),
        // Lent as empty text when it plays nothing.
        shown
            .shows
            .plays
            .as_ref()
            .map_or_else(String::new, |(_, file)| file.clone()),
        shown.shows.poster.clone(),
    ];
    let laid = page_players().lay(index, &block, shown, top, side, column);
    if laid.slept {
        close_page_preview();
    }
    NrPagePlayer {
        kind: match laid.kind {
            Kind::Words => NrPlayerKind::Words,
            Kind::Video => NrPlayerKind::Video,
            Kind::Sound => NrPlayerKind::Sound,
        },
        block: NrText::borrowing(&lent[0]),
        frame: nr_rect(laid.frame),
        picture: nr_rect(laid.picture),
        controls: laid.controls,
        changed: laid.changed,
        slept: laid.slept,
        words: NrText::borrowing(&lent[1]),
        file: NrText::borrowing(&lent[2]),
        poster: NrText::borrowing(&lent[3]),
    }
}

/// The page's layout is done: the players of blocks not laid out (`seen`, ids one per line)
/// are let go, the one awake put to rest and its file closed. Returns the blocks let go, one per
/// line. Valid until the next call that lends text.
///
/// # Safety
/// `seen` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_page_players_forget(seen: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let seen = ids_of(&unsafe { text_of(seen) });
    let (awake, gone) = {
        let mut players = page_players();
        let awake = players.awake().map(str::to_owned);
        (awake, players.forget_except(&seen))
    };
    if awake.is_some_and(|block| gone.contains(&block)) {
        close_page_preview();
    }
    lend(gone.join("\n"))
}

/// The block whose player is awake; empty when none is. Valid until the next call that lends
/// text.
#[no_mangle]
pub extern "C" fn nr_page_player_awake() -> NrText {
    lend(page_players().awake().unwrap_or_default().to_owned())
}

/// What waking a page's player opened.
#[repr(C)]
pub struct NrPlayerWake {
    /// Its file is the preview's now, paused where it was left.
    pub opened: bool,
    /// A picture, which the shell gives a layer where the picture sits; else a sound.
    pub video: bool,
}

/// Wakes `block`'s player: its file opens in the preview, in place of whatever it held, where it
/// was left, muted when the element says. The one awake before must be put to rest first
/// (`nr_page_player_sleep`). Nothing opens when it plays nothing or its file has gone.
///
/// # Safety
/// `block` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_page_player_wake(block: *const c_char) -> NrPlayerWake {
    // SAFETY: the caller passes a NUL-terminated string.
    let block = unsafe { text_of(block) };
    let file = page_players().file_of(&block).map(str::to_owned);
    let path = file
        .and_then(|file| with_columns(|columns| columns.workspace().path_of(&file).ok()).flatten());
    let Some((waking, path)) = page_players().wake(&block, path.is_some()).zip(path) else {
        return NrPlayerWake {
            opened: false,
            video: false,
        };
    };
    let what = if waking.video {
        crate::preview::Playing::Video
    } else {
        crate::preview::Playing::Sound
    };
    let number = hold_preview(|| {
        let preview = Preview::start_as(path.to_string_lossy().into_owned(), what, waking.start_at);
        if waking.muted {
            preview.set_muted(true);
        }
        preview
    });
    PAGE_PREVIEW.store(number, std::sync::atomic::Ordering::Relaxed);
    NrPlayerWake {
        opened: true,
        video: waking.video,
    }
}

/// Puts the page's player awake back to rest where it was, and closes its file. Returns its
/// block; empty when none was awake. Valid until the next call that lends text.
#[no_mangle]
pub extern "C" fn nr_page_player_sleep() -> NrText {
    // Another opener may have taken the preview since (a video selected in a column): the
    // player then rests at its start and what plays now is left playing.
    let ours = PAGE_PREVIEW.load(std::sync::atomic::Ordering::Relaxed)
        == PREVIEW_HELD.load(std::sync::atomic::Ordering::Relaxed);
    let info = ours
        .then(|| with_preview(Preview::info))
        .flatten()
        .unwrap_or_default();
    let slept = page_players().sleep(info.position_seconds, info.ended);
    if slept.is_some() {
        close_page_preview();
    }
    lend(slept.unwrap_or_default())
}

/// What a look at the page's player awake asks of the shell.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrPlayerFollow {
    /// None is awake.
    Nothing,
    /// It goes to rest: the shell calls `nr_page_player_sleep`.
    Sleep,
    /// It carries on and its controls are drawn again (a `loop` played to its end plays again).
    Draw,
}

/// A look at the page's player awake, the page `visible` in a window or not.
#[no_mangle]
pub extern "C" fn nr_page_player_follow(visible: bool) -> NrPlayerFollow {
    use crate::page::players::Follow;
    let ended = with_preview(Preview::info).is_some_and(|info| info.ended);
    let follow = page_players().follow(visible, ended);
    match follow {
        None => NrPlayerFollow::Nothing,
        Some(Follow::Sleep) => NrPlayerFollow::Sleep,
        Some(Follow::PlayAgain) => {
            with_preview(Preview::toggle_playing);
            NrPlayerFollow::Draw
        }
        Some(Follow::Draw) => NrPlayerFollow::Draw,
    }
}

/// For a check (`previewpage`): what the open page's players show and do, as one line of JSON
/// (`page::players::PagePlayers::report`). The shell says where the page's top-left corner is
/// from the window's, whether its sound output runs, the blocks selected (`chosen_start` -1:
/// none), the block being typed in (-1: none) and what has the keys. It reads and changes
/// nothing. Valid until the next call that lends text.
///
/// # Safety
/// `keys` must point to a NUL-terminated string.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn nr_page_players_report(
    corner_x: f64,
    corner_y: f64,
    output: bool,
    chosen_start: i64,
    chosen_end: i64,
    editing: i64,
    keys: *const c_char,
) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let keys = unsafe { text_of(keys) };
    let blocks = app()
        .page
        .relock()
        .as_ref()
        .map_or(-1, |page| page.blocks.len() as i64);
    let stats = with_preview(Preview::audio_stats).unwrap_or_default();
    let observed = crate::page::players::Observed {
        corner: (corner_x, corner_y),
        preview: with_preview(Preview::info),
        menu: with_preview(Preview::menu).unwrap_or_default(),
        sound_frames: stats.frames,
        sound_peak: stats.peak,
        output,
        chosen: (chosen_start >= 0).then_some((chosen_start, chosen_end)),
        editing,
        keys,
        blocks,
    };
    lend(page_players().report(&observed))
}

/// Goes up each time the theme's colours change, so the shell's cache of them follows
/// (`nr_theme_revision`).
static THEME_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many times the theme's colours have changed: a shell drops the colours it keeps when
/// this differs from the count it kept them at.
#[no_mangle]
pub extern "C" fn nr_theme_revision() -> u64 {
    THEME_REVISION.load(std::sync::atomic::Ordering::Acquire)
}

/// Derives the theme in effect from the appearance choices and bumps the revision views
/// redraw on. Running terminals take its colours too.
pub(crate) fn apply_appearance() {
    let theme = {
        let appearance = app().appearance.relock();
        let mode = appearance.0.mode(appearance.1);
        derive(&appearance.0.theme_of(mode).appearance(), mode)
    };
    if let Some(theme) = theme {
        *app().theme.relock() = theme;
        THEME_REVISION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let palette = terminal_palette();
        let sessions = app().sessions.relock();
        for session in sessions.values() {
            session.set_palette(palette);
        }
    }
}

pub(crate) fn with_appearance<T>(
    action: impl FnOnce(&mut crate::settings::appearance::Settings) -> T,
) -> T {
    let mut appearance = app().appearance.relock();
    action(&mut appearance.0)
}

/// Changes the appearance choices; when `action` says they changed, keeps them and applies
/// the theme they give. Returns whether they changed.
pub(crate) fn change_appearance(
    action: impl FnOnce(&mut crate::settings::appearance::Settings) -> bool,
) -> bool {
    let changed = with_appearance(|settings| {
        let changed = action(settings);
        if changed {
            if let Err(error) = settings.save() {
                eprintln!("the appearance was not kept: {error}");
            }
        }
        changed
    });
    if changed {
        apply_appearance();
    }
    changed
}

fn mode_of(dark: bool) -> Mode {
    if dark {
        Mode::Dark
    } else {
        Mode::Light
    }
}

/// Applies the appearance at launch: the choices kept in the app's settings folder
/// (`paths::Paths::settings`), or a check's own look (`NR_THEME`), for which nothing kept is
/// read: its colours when it names them, and the look it names shown as chosen in Settings.
/// `system_is_dark` says which look the system is showing, for a person who follows it.
#[no_mangle]
pub extern "C" fn nr_appearance_start(system_is_dark: bool) {
    let Some(check) = &crate::launch::get().theme else {
        let file = crate::paths::get().settings_file;
        let settings = crate::settings::appearance::Settings::open(&file);
        *app().appearance.relock() = (settings, system_is_dark);
        apply_appearance();
        watch_settings_file(file);
        return;
    };
    with_appearance(|settings| settings.check_choice(check.dark));
    if let Some(colours) = &check.colours {
        let appearance = Appearance {
            background: colours.background.clone(),
            foreground: colours.foreground.clone(),
            accent_color: colours.accent.clone(),
            cta_color: colours.call_to_action.clone(),
            contrast: colours.contrast,
        };
        let mode = mode_of(colours.dark);
        if let Some(theme) = derive(&appearance, mode) {
            *app().theme.relock() = theme;
        }
    }
}

/// Watches the settings file's folder for as long as the app runs, so an edit by the person or
/// their agent applies at once (`settings::file`). Editors save by writing a new file and moving
/// it over the old one, which a watch of the file alone would lose.
fn watch_settings_file(file: PathBuf) {
    static WATCH: std::sync::OnceLock<std::sync::Mutex<crate::drive::watch::FolderWatch>> =
        std::sync::OnceLock::new();
    let Some(folder) = file.parent().map(Path::to_path_buf) else {
        return;
    };
    if std::fs::create_dir_all(&folder).is_err() {
        return;
    }
    let read = file.clone();
    let Ok(mut watch) =
        crate::drive::watch::FolderWatch::start(move || settings_file_changed(&read))
    else {
        return;
    };
    watch.watch(&[folder]);
    let _ = WATCH.set(std::sync::Mutex::new(watch));
}

/// The settings file changed on disk: its appearance applies when it differs from the one in
/// effect. A file broken by hand is logged and leaves the appearance as it was; the
/// drive it names is read at the next start (`selection`), since changing the drive ends every
/// terminal, the agent's that wrote it among them.
fn settings_file_changed(file: &Path) {
    let read = match crate::settings::appearance::Settings::read(file) {
        Ok(read) => read,
        Err(why) => {
            crate::diagnostics::write_with(
                crate::diagnostics::Level::Warn,
                "the settings file was not applied",
                &serde_json::json!({ "why": why }),
            );
            eprintln!("the settings file was not applied: {why}");
            return;
        }
    };
    let changed = with_appearance(|current| {
        let differs =
            current.theme != read.theme || current.light != read.light || current.dark != read.dark;
        if differs {
            current.theme = read.theme;
            current.light = read.light.clone();
            current.dark = read.dark.clone();
        }
        differs
    });
    if changed {
        apply_appearance();
        crate::notify_changed();
    }
}

/// Whether the theme in effect, the one every view is drawn in, is the dark one: a check's
/// own look (`NR_THEME`) as much as a person's choice.
#[no_mangle]
pub extern "C" fn nr_appearance_is_dark() -> bool {
    app().theme.relock().mode == Mode::Dark
}

/// Fills `out` with a theme colour by its token name (`background`, `foreground`, `frame`,
/// `border`, `muted-foreground`, …). Returns false for a name that is not a token.
///
/// # Safety
/// `name` must point to a NUL-terminated string and `out` to storage for one `NrColor`.
#[no_mangle]
pub unsafe extern "C" fn nr_theme_color(name: *const c_char, out: *mut NrColor) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let name = unsafe { text_of(name) };
    let Some(token) = crate::theme::Token::named(&name) else {
        return false;
    };
    let theme = app().theme.relock();
    let (color, alpha) = theme.token(token);
    let [red, green, blue] = oklch_to_srgb8(color);
    // SAFETY: the caller passes storage for one colour.
    unsafe {
        out.write(NrColor {
            red: f32::from(red) / 255.0,
            green: f32::from(green) / 255.0,
            blue: f32::from(blue) / 255.0,
            alpha: alpha as f32,
        });
    }
    true
}

/// The terminal's view changed size or cell. The grid, and the process, change only when a
/// whole cell is gained or lost; the shell calls this at most once a frame.
#[no_mangle]
pub extern "C" fn nr_terminal_resize(number: u64, view: NrTerminalView) {
    with_shown(number, |terminal| terminal.resize(view.size()));
}

/// Sends `length` bytes to the terminal's process, as typed.
///
/// # Safety
/// `bytes` must point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_input(number: u64, bytes: *const u8, length: usize) {
    // SAFETY: the caller passes `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    with_shown(number, |terminal| terminal.input(bytes));
}

/// A key pressed on the terminal. Returns whether the terminal sent it as a key; a key that
/// types (Space included) is not sent, and goes on to the input system as text.
#[no_mangle]
pub extern "C" fn nr_terminal_key(number: u64, press: NrKeyPress) -> bool {
    let Some(key) = crate::terminal::input::Key::from_press(press.key) else {
        return false;
    };
    with_shown(number, |terminal| {
        terminal.key(key, crate::terminal::input::Modifiers::held(press.held))
    });
    true
}

/// What the mouse did over the terminal.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMouseAction {
    Pressed,
    Dragged,
    Released,
}

/// A mouse button.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMouseButton {
    Left,
    Middle,
    Right,
}

/// The mouse over the terminal, at `x`, `y` in the view's pixels from its top left. `clicks`
/// is 2 for a double click and 3 for a triple; `modifiers` the keys held (`NR_HELD_*`).
#[no_mangle]
pub extern "C" fn nr_terminal_mouse(
    number: u64,
    action: NrMouseAction,
    button: NrMouseButton,
    x: f32,
    y: f32,
    clicks: u32,
    modifiers: u32,
) {
    use crate::terminal::input::MouseAction;
    let action = match action {
        NrMouseAction::Pressed => MouseAction::Press,
        NrMouseAction::Dragged => MouseAction::Drag,
        NrMouseAction::Released => MouseAction::Release,
    };
    // xterm's numbers for the buttons.
    let button = match button {
        NrMouseButton::Left => 0,
        NrMouseButton::Middle => 1,
        NrMouseButton::Right => 2,
    };
    with_shown(number, |terminal| {
        terminal.mouse(
            action,
            button,
            x,
            y,
            clicks,
            crate::terminal::input::Modifiers::held(modifiers),
        )
    });
}

/// The wheel or trackpad over the terminal at `x`, `y`. `delta` is pixels of the view when
/// `precise` (a trackpad) and wheel notches when not; positive shows older output.
pub(crate) fn terminal_scroll(
    number: u64,
    delta: f32,
    precise: bool,
    x: f32,
    y: f32,
    modifiers: crate::terminal::input::Modifiers,
) {
    with_shown(number, |terminal| {
        terminal.scroll(delta, precise, x, y, modifiers)
    });
}

/// The terminal's selected text, empty when nothing is selected, for the pasteboard.
#[no_mangle]
pub extern "C" fn nr_terminal_copy(number: u64) -> NrText {
    lend(with_shown(number, crate::terminal::Terminal::copy).unwrap_or_default())
}

/// Selects everything in the terminal, its scrollback included.
#[no_mangle]
pub extern "C" fn nr_terminal_select_all(number: u64) {
    with_shown(number, crate::terminal::Terminal::select_all);
}

/// Pastes `length` bytes of UTF-8 into the terminal, marked as a paste when the program
/// asked for that.
///
/// # Safety
/// `bytes` must point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_paste(number: u64, bytes: *const u8, length: usize) {
    // SAFETY: the caller passes `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let text = String::from_utf8_lossy(bytes);
    with_shown(number, |terminal| terminal.paste(&text));
}

/// Pastes `paths` into `terminal` as a terminal takes files dropped on it (`reference_text`),
/// so its program reads them as references; an agent attaches a picture pasted this way.
/// Nothing is entered: the person adds their words and presses Return. False when there are
/// no files, or the terminal has no program running to read them.
fn hand_references(terminal: &Terminal, paths: &[PathBuf]) -> bool {
    if paths.is_empty() || !terminal.activity().takes_references() {
        return false;
    }
    terminal.paste(&crate::terminal::input::reference_text(paths));
    true
}

/// Where the drive's files `ids` are now; one that has left the drive is left out.
fn paths_of(ids: &[String]) -> Vec<PathBuf> {
    with_columns(|columns| {
        let workspace = columns.workspace();
        ids.iter()
            .filter_map(|id| workspace.path_of(id).ok())
            .collect()
    })
    .unwrap_or_default()
}

/// Files from outside the app, the newline-joined `paths`, were dropped on the terminal on
/// screen: its program is handed them (`hand_references`). False when it took nothing.
///
/// # Safety
/// `paths` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_drop(number: u64, paths: *const c_char) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let dropped: Vec<PathBuf> = ids_of(&unsafe { text_of(paths) })
        .into_iter()
        .map(PathBuf::from)
        .collect();
    with_shown(number, |terminal| hand_references(terminal, &dropped)).unwrap_or(false)
}

/// The drive's files `ids` (one per line) were dropped on the terminal on screen: its program
/// is handed them (`hand_references`). False when it took nothing.
///
/// # Safety
/// `ids` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_refer(number: u64, ids: *const c_char) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let dropped = paths_of(&ids_of(&unsafe { text_of(ids) }));
    with_shown(number, |terminal| hand_references(terminal, &dropped)).unwrap_or(false)
}

/// Pastes the paths of the drive's files `ids` into the session of the terminal file
/// `terminal_id`, as dropping files on a terminal does, so its program reads them as
/// references; an agent attaches a picture pasted this way. Nothing is entered: the person adds
/// their words and presses Return. False when that terminal has no program running to read
/// them, or none of the files is on the drive now.
pub(crate) fn reference_files(terminal_id: &str, ids: &[String]) -> bool {
    let paths = paths_of(ids);
    let sessions = app().sessions.relock();
    sessions
        .get(terminal_id)
        .is_some_and(|session| hand_references(session, &paths))
}

/// Tells the terminal whether it has the keyboard: its cursor is solid and blinks when it
/// does, an outline when it does not.
#[no_mangle]
pub extern "C" fn nr_terminal_focus(number: u64, focused: bool) {
    if focused {
        app().shown_terminals.relock().bring_front(number);
    }
    with_shown(number, |terminal| terminal.focus(focused));
}

/// The text an input method is composing, drawn at the cursor until it is committed; a
/// length of zero ends it.
///
/// # Safety
/// `bytes` must point to `length` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_compose(number: u64, bytes: *const u8, length: usize) {
    // SAFETY: the caller passes `length` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    let text = String::from_utf8_lossy(bytes);
    with_shown(number, |terminal| terminal.compose(&text));
}

/// Fills `out` with where the terminal's cursor is drawn: x, y, width, height in the view's
/// pixels from its top left, for placing an input method's window. False with no terminal.
///
/// # Safety
/// `out` must point to storage for four floats.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_cursor_rect(number: u64, out: *mut f32) -> bool {
    let Some(rect) = with_shown(number, crate::terminal::Terminal::cursor_rect) else {
        return false;
    };
    // SAFETY: the caller passes storage for four floats.
    unsafe { std::ptr::copy_nonoverlapping(rect.as_ptr(), out, 4) };
    true
}

/// What a piece of a terminal's frame is (`NrTerminalPiece`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrTerminalPieceKind {
    /// A filled rectangle: a cell's background, a line, the cursor.
    Fill,
    /// Text set in the terminal font from a cell's top left, one character to a cell.
    Text,
}

/// One thing a terminal draws, in the view's pixels from its top left. A fill covers `width` ×
/// `height`. Text spans `cells` cells on a line `height` high; it is `text_length` bytes of the
/// frame's text from `text_from`, set in bold or italic as asked.
#[repr(C)]
pub struct NrTerminalPiece {
    pub kind: NrTerminalPieceKind,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: NrColor,
    pub cells: u32,
    pub text_from: usize,
    pub text_length: usize,
    pub bold: bool,
    pub italic: bool,
}

/// A terminal's frame: its background, then `count` pieces drawn in order over it. `text` holds
/// every text piece's bytes. `redraw_after_ms` is when to draw again for the cursor's blink; 0
/// when nothing changes until `nr_terminal_watch_frames` says so.
#[repr(C)]
pub struct NrTerminalFrame {
    pub background: NrColor,
    pub pieces: *const NrTerminalPiece,
    pub count: usize,
    pub text: NrText,
    pub redraw_after_ms: u32,
}

fn ink_color(ink: crate::terminal::frame::Ink) -> NrColor {
    NrColor {
        red: f32::from(ink.rgb.r) / 255.0,
        green: f32::from(ink.rgb.g) / 255.0,
        blue: f32::from(ink.rgb.b) / 255.0,
        alpha: ink.alpha,
    }
}

/// A piece of a frame as the boundary hands it over; text sits on lines `line_height` high.
fn boundary_piece(piece: &crate::terminal::frame::Piece, line_height: f32) -> NrTerminalPiece {
    use crate::terminal::frame::Piece;
    match piece {
        Piece::Fill { rect, ink } => NrTerminalPiece {
            kind: NrTerminalPieceKind::Fill,
            x: rect[0],
            y: rect[1],
            width: rect[2],
            height: rect[3],
            color: ink_color(*ink),
            cells: 0,
            text_from: 0,
            text_length: 0,
            bold: false,
            italic: false,
        },
        Piece::Text {
            origin,
            cells,
            text,
            ink,
            face,
        } => NrTerminalPiece {
            kind: NrTerminalPieceKind::Text,
            x: origin[0],
            y: origin[1],
            width: 0.0,
            height: line_height,
            color: ink_color(*ink),
            cells: *cells as u32,
            text_from: text.start,
            text_length: text.len(),
            bold: face.is_bold(),
            italic: face.is_italic(),
        },
    }
}

/// A terminal frame's pieces and text, lent until the next `nr_terminal_frame`.
struct LentFrame {
    pieces: Vec<NrTerminalPiece>,
    text: String,
}

/// Fills `out` with what the terminal on screen as `number` shows now, read from its emulator
/// as the shell draws it with the rest of the window. False with no terminal. The pieces and
/// text are lent until the next call.
///
/// # Safety
/// `out` must point to writable storage for one `NrTerminalFrame`.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_frame(number: u64, out: *mut NrTerminalFrame) -> bool {
    static LENT: std::sync::Mutex<Option<LentFrame>> = std::sync::Mutex::new(None);
    let Some((frame, turn)) = with_shown(number, Terminal::frame).flatten() else {
        return false;
    };
    let line_height = frame.cursor_rect[3];
    let mut lent = LENT.relock();
    let lent = lent.insert(LentFrame {
        pieces: frame
            .pieces
            .iter()
            .map(|piece| boundary_piece(piece, line_height))
            .collect(),
        text: frame.text,
    });
    let redraw_after_ms = turn.map_or(0, |turn| {
        u32::try_from(turn.as_millis()).unwrap_or(u32::MAX).max(1)
    });
    // SAFETY: the caller passes storage for one frame.
    unsafe {
        out.write(NrTerminalFrame {
            background: ink_color(crate::terminal::frame::Ink {
                rgb: frame.background,
                alpha: 1.0,
            }),
            pieces: lent.pieces.as_ptr(),
            count: lent.pieces.len(),
            text: NrText::borrowing(&lent.text),
            redraw_after_ms,
        });
    }
    true
}

/// Has `changed` called, from any thread, when a terminal on screen has something new to draw,
/// so the shell draws its window again: once until it takes that terminal's next frame
/// (`nr_terminal_frame`). Once, at launch.
#[no_mangle]
pub extern "C" fn nr_terminal_watch_frames(changed: extern "C" fn()) {
    crate::terminal::watch_frames(changed);
}

/// How many sessions are working or waiting for the person: what quitting would interrupt.
pub(crate) fn sessions_at_work() -> usize {
    use crate::terminal::activity::Activity;
    let sessions = app().sessions.relock();
    sessions
        .values()
        .filter_map(|session| {
            crate::sessions::listed_activity(
                session.activity(),
                session.agent().is_some(),
                session.busy(),
            )
        })
        .filter(|activity| matches!(activity, Activity::Working | Activity::NeedsYou))
        .count()
}

/// What the session of the terminal file `id` is doing, as the sessions list says it (a shell
/// running a program is at work); `None` when none runs.
pub(crate) fn session_activity(id: &str) -> Option<crate::terminal::activity::Activity> {
    let sessions = app().sessions.relock();
    sessions.get(id).map(|session| {
        crate::sessions::listed_activity(
            session.activity(),
            session.agent().is_some(),
            session.busy(),
        )
        .unwrap_or_else(|| session.activity())
    })
}

/// Ends the session of the terminal file `id`, as a session's X or End Session does: its
/// program stops and the tabs showing it close (`Columns::close_tabs_of`). Its file stays, and
/// opening it starts a new session.
pub(crate) fn end_session(id: &str) {
    let session = app().sessions.relock().remove(id);
    if let Some(session) = session {
        session.stop();
    }
    crate::changes::ffi::unfollow_terminal(id);
    with_columns(|columns| columns.close_tabs_of(id));
    trash_if_throwaway(id);
    watch_open_columns();
}

/// Moves the terminal file `id` to the Trash when it is a throwaway
/// (`Columns::is_throwaway_terminal`): its session has ended, and nothing kept it. Returns
/// whether it went.
fn trash_if_throwaway(id: &str) -> bool {
    let label = crate::strings::text(
        interface_language(),
        crate::strings::key::TERMINAL_SHELL_LABEL,
    );
    with_columns(|columns| {
        if !columns.is_throwaway_terminal(id, label) {
            return false;
        }
        match columns.trash(&[id.to_owned()]) {
            Ok(_) => true,
            Err(error) => {
                eprintln!("the throwaway terminal was not moved to the Trash: {error}");
                false
            }
        }
    })
    .unwrap_or(false)
}

/// Whether the session of the terminal file `id` runs nothing but its shell at the prompt, or
/// has ended: no agent, no program at work, nothing waiting to be seen. Closing its last tab
/// ends it, as nothing in it would be lost.
fn session_at_rest(id: &str) -> bool {
    let sessions = app().sessions.relock();
    sessions.get(id).is_some_and(|session| {
        crate::sessions::listed_activity(
            session.activity(),
            session.agent().is_some(),
            session.busy(),
        )
        .is_none()
    })
}

/// Keeps the open tabs for the next launch (`Columns::keep_tabs`). Call when the app quits,
/// before `nr_terminals_quit` moves throwaway terminals to the Trash.
#[no_mangle]
pub extern "C" fn nr_tabs_keep() {
    with_columns(Columns::keep_tabs);
}

/// Ends every terminal session, keeping each one's output to show above its next session
/// (`terminal::transcript`). Call when the app quits.
#[no_mangle]
pub extern "C" fn nr_terminals_quit() {
    let app = app();
    // Read before the sessions' lock, which the columns' lock never waits under.
    let metadata = with_columns(|columns| columns.workspace().metadata_directory().to_path_buf());
    app.shown_terminals.relock().clear();
    let sessions: Vec<(String, Terminal)> = app.sessions.relock().drain().collect();
    let ended: Vec<String> = sessions.iter().map(|(id, _)| id.clone()).collect();
    // Two of an agent in one folder can't each resume its latest conversation (`unclaimed`).
    let resumes = crate::terminal::resume::unclaimed(
        sessions
            .iter()
            .filter_map(|(id, session)| Some((id.clone(), session.resume_command()?)))
            .collect(),
    );
    for (id, session) in sessions {
        if let Some(metadata) = &metadata {
            keep_transcript(metadata, &id, &session.transcript());
            let resume = resumes.iter().find(|(each, _)| *each == id);
            keep_resume(
                metadata,
                &id,
                resume.map(|(_, resume)| resume.command.as_str()),
            );
        }
        session.stop();
    }
    for id in ended {
        crate::changes::ffi::unfollow_terminal(&id);
        // A throwaway terminal goes to the Trash, and its output with it.
        if trash_if_throwaway(&id) {
            if let Some(metadata) = &metadata {
                keep_transcript(metadata, &id, "");
                keep_resume(metadata, &id, None);
            }
        }
    }
    crate::changes::ffi::save_now();
}

/// Keeps the command that resumes terminal `id`'s agent, or clears it for `None`
/// (`transcript::keep_resume`); a failure is only reported.
fn keep_resume(metadata: &Path, id: &str, command: Option<&str>) {
    if let Err(error) = crate::terminal::transcript::keep_resume(metadata, id, command) {
        eprintln!("the terminal's agent was not kept to resume: {error}");
    }
}

/// Keeps terminal `id`'s `text` (`transcript::keep`); a failure is only reported.
fn keep_transcript(metadata: &Path, id: &str, text: &str) {
    if let Err(error) = crate::terminal::transcript::keep(metadata, id, text) {
        eprintln!("the terminal's output was not kept: {error}");
    }
}

/// Whether a press that has travelled (`dx`, `dy`) points on a screen of `scale` pixels a
/// point has become a drag (`row_pointer::drag_starts`).
#[no_mangle]
pub extern "C" fn nr_drag_starts(dx: f64, dy: f64, scale: f64) -> bool {
    crate::list::row_pointer::drag_starts(dx, dy, scale)
}

/// One of the app's durations, delays and distances (`motion`), in milliseconds or points.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMotion {
    /// A popup's move.
    Popup,
    /// A dialog's panel.
    Dialog,
    /// A dialog's shade.
    Shade,
    Hover,
    TooltipDelay,
    /// The tooltip's skip window.
    TooltipSkip,
    SubmenuDelay,
    /// The slide's distance, in points.
    Slide,
}

#[no_mangle]
pub extern "C" fn nr_motion_number(which: NrMotion) -> f64 {
    use crate::motion as m;
    match which {
        NrMotion::Popup => m::POPUP_MS,
        NrMotion::Dialog => m::DIALOG_MS,
        NrMotion::Shade => m::SHADE_MS,
        NrMotion::Hover => m::HOVER_MS,
        NrMotion::TooltipDelay => m::TOOLTIP_DELAY_MS,
        NrMotion::TooltipSkip => m::TOOLTIP_SKIP_MS,
        NrMotion::SubmenuDelay => m::SUBMENU_DELAY_MS,
        NrMotion::Slide => m::SLIDE,
    }
}

/// Where a wheel or trackpad event goes (`scroll::route::Route`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrRoute {
    /// The scroller's own.
    Own,
    /// Handed on to the scroller around it.
    HandOn,
    /// Held until the gesture travels.
    Hold,
}

/// What a scroller remembers of a gesture from one event to the next: the core's own type,
/// which is `#[repr(C)]`.
pub use crate::scroll::route::Latch as NrLatch;

/// Where an event goes, the latch to hand back with the next, and the route's name for a
/// check's trace.
#[repr(C)]
pub struct NrScrollRoute {
    pub route: NrRoute,
    pub latch: NrLatch,
    pub name: NrText,
}

/// Which scroller takes a wheel or trackpad event (`scroll::route::route`). `latch` is what
/// the scroller remembered from the event before (`Free` at first).
#[no_mangle]
pub extern "C" fn nr_scroll_route(
    dx: f64,
    dy: f64,
    phase: u32,
    momentum: u32,
    sideways: bool,
    latch: NrLatch,
) -> NrScrollRoute {
    use crate::scroll::route::{Route, Scroll};
    let (route, latch) = crate::scroll::route::route(
        Scroll {
            dx,
            dy,
            phase,
            momentum,
        },
        sideways,
        latch,
    );
    let (route, name) = match route {
        Route::Own => (NrRoute::Own, "own"),
        Route::HandOn => (NrRoute::HandOn, "handon"),
        Route::Hold => (NrRoute::Hold, "hold"),
    };
    NrScrollRoute {
        route,
        latch,
        name: NrText::borrowing(name),
    }
}

/// Where a scroller the app draws is scrolled to from `offset` after a wheel's travel `delta`
/// (positive further in) counted in `unit`, showing `shown` of `length`
/// (`scroll::route::wheel_scrolled`).
#[no_mangle]
pub extern "C" fn nr_wheel_scrolled(
    offset: f64,
    delta: f64,
    unit: u32,
    shown: f64,
    length: f64,
) -> f64 {
    crate::scroll::route::wheel_scrolled(offset, delta, unit, shown, length)
}

/// A wheel's travel `delta` counted in `unit`, in points (`scroll::route::wheel_points`), for a
/// scroller the app draws whose offset the core holds to its ends as it lays it out.
#[no_mangle]
pub extern "C" fn nr_wheel_points(delta: f64, unit: u32) -> f64 {
    crate::scroll::route::wheel_points(delta, unit)
}

/// One of the numbers the video controls are drawn with (`player::video_controls`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrControlsNumber {
    /// A glyph's width.
    Glyph,
    /// A knob's.
    Knob,
    /// The bar's ground strength.
    TrackAlpha,
    /// The loaded part's.
    LoadedAlpha,
    /// How many stops the shade has.
    ShadeStops,
    /// The strength of a ground at its fullest, which the scrub bar's preview waits on.
    GroundAlpha,
    /// The scrub bar's preview's corners.
    PreviewCorner,
    /// The room either side of the words in the preview's pill of time.
    PreviewPad,
    /// How many moments the preview shows across the length.
    PreviewSteps,
    /// How many moments either side of the one shown are read ahead.
    PreviewAhead,
}

#[no_mangle]
pub extern "C" fn nr_video_controls_number(which: NrControlsNumber) -> f64 {
    use crate::preview::player::video_controls as controls;
    match which {
        NrControlsNumber::Glyph => controls::GLYPH,
        NrControlsNumber::Knob => controls::KNOB,
        NrControlsNumber::TrackAlpha => controls::TRACK_ALPHA,
        NrControlsNumber::LoadedAlpha => controls::LOADED_ALPHA,
        NrControlsNumber::ShadeStops => controls::SHADE_STOPS.len() as f64,
        NrControlsNumber::GroundAlpha => controls::VOLUME_GROUND_ALPHA,
        NrControlsNumber::PreviewCorner => controls::PREVIEW_CORNER,
        NrControlsNumber::PreviewSteps => controls::PREVIEW_STEPS,
        NrControlsNumber::PreviewAhead => controls::PREVIEW_AHEAD,
        NrControlsNumber::PreviewPad => controls::PREVIEW_PAD,
    }
}

/// Stop `index` of the shade under the controls: how far down the shade it is, or with
/// `strength` how black.
#[no_mangle]
pub extern "C" fn nr_video_controls_shade(index: usize, strength: bool) -> f64 {
    crate::preview::player::video_controls::SHADE_STOPS
        .get(index)
        .map_or(0.0, |stop| if strength { stop.1 } else { stop.0 })
}

/// The menu part-way through opening: `row` -1 the sheet (`out`: its size about its bottom
/// right corner, its strength), else a row of the main menu counted from the bottom (`out`:
/// its strength, how far below its place it is) (`player::video_controls::menu_entrance`).
///
/// # Safety
/// `out` must point to writable storage for 2 numbers.
#[no_mangle]
pub unsafe extern "C" fn nr_video_menu_entrance(row: i32, since_ms: f64, out: *mut f64) {
    use crate::preview::player::video_controls::{menu_entrance, menu_row_entrance};
    if out.is_null() {
        return;
    }
    let (a, b) = if row < 0 {
        menu_entrance(since_ms)
    } else {
        menu_row_entrance(row as usize, since_ms)
    };
    // SAFETY: the caller passes storage for 2 numbers.
    unsafe { std::slice::from_raw_parts_mut(out, 2) }.copy_from_slice(&[a, b]);
}

/// Which of the player's menus is open.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMenuShown {
    Closed,
    /// The menu.
    Main,
    /// The list of speeds.
    Speeds,
}

/// The menu behind the opened video's three dots, as it stands over a picture.
#[repr(C)]
pub struct NrVideoMenu {
    pub kind: NrMenuShown,
    /// How many rows it has, and which has the keyboard.
    pub rows: u32,
    pub focus: u32,
    /// The row of the list of speeds that is ticked.
    pub ticked: u32,
    /// The sheet: left, top, width, height.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The menu over a picture `width` by `height` (`player::video_controls::Menu`).
#[no_mangle]
pub extern "C" fn nr_video_menu(width: f64, height: f64) -> NrVideoMenu {
    use crate::preview::player::video_controls::{ticked_speed, Menu};
    let (menu, rate) = with_preview(|preview| (preview.menu(), preview.info().rate))
        .unwrap_or((Menu::Closed, 1.0));
    let sheet = menu.frame(width, height);
    NrVideoMenu {
        kind: menu.kind(),
        rows: menu.rows() as u32,
        focus: menu.focus().unwrap_or(0) as u32,
        ticked: ticked_speed(rate) as u32 + 1,
        x: sheet.x,
        y: sheet.y,
        width: sheet.width,
        height: sheet.height,
    }
}

/// Row `index` of the open menu: `out` takes its top and its height over the picture (it may
/// lie outside the sheet, which clips it). False when there is no such row.
///
/// # Safety
/// `out` must point to writable storage for 2 numbers.
#[no_mangle]
pub unsafe extern "C" fn nr_video_menu_row(
    index: u32,
    width: f64,
    height: f64,
    out: *mut f64,
) -> bool {
    let Some(row) =
        with_preview(|preview| preview.menu().row(index as usize, width, height)).flatten()
    else {
        return false;
    };
    if !out.is_null() {
        // SAFETY: the caller passes storage for 2 numbers.
        unsafe { std::slice::from_raw_parts_mut(out, 2) }.copy_from_slice(&[row.y, row.height]);
    }
    true
}

/// The picture row `index` of the open menu shows at its left (`glyph::MediaGlyph`); empty for
/// none. Valid for as long as the app runs.
#[no_mangle]
pub extern "C" fn nr_video_menu_glyph(index: u32) -> NrText {
    let glyph = with_preview(Preview::menu).and_then(|menu| menu.row_glyph(index as usize));
    NrText::borrowing(glyph.map_or("", crate::glyph::MediaGlyph::name))
}

/// What row `index` of the open menu says; with `second`, its second line ("Off" under
/// Captions), empty for a row that has none. Valid until the next call that lends text.
#[no_mangle]
pub extern "C" fn nr_video_menu_label(index: u32, second: bool) -> NrText {
    let menu =
        with_preview(Preview::menu).unwrap_or(crate::preview::player::video_controls::Menu::Closed);
    lend(
        menu.label(index as usize, second, interface_language())
            .unwrap_or("")
            .to_owned(),
    )
}

/// One of the numbers the player's menu is drawn with (`player::video_controls`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMenuNumber {
    Corner,
    /// Its shadow's drop.
    ShadowDrop,
    ShadowBlur,
    /// Its shadow's strength.
    ShadowAlpha,
    /// The grey of the row in focus.
    FocusGrey,
    /// The words' strength.
    InkAlpha,
    /// The second line's.
    QuietAlpha,
    /// The font's size.
    Font,
    /// A line's height.
    Line,
    /// Where a row's picture starts.
    IconX,
    /// The picture's size.
    Icon,
    /// Where the menu's words start.
    WordsX,
    /// Where a choice's words start.
    ChoiceX,
    /// The tick's size.
    Tick,
    /// How far short of the right edge it ends.
    TickRight,
}

#[no_mangle]
pub extern "C" fn nr_video_menu_number(which: NrMenuNumber) -> f64 {
    use crate::preview::player::video_controls as controls;
    use NrMenuNumber as N;
    match which {
        N::Corner => controls::MENU_CORNER,
        N::ShadowDrop => controls::MENU_SHADOW.0,
        N::ShadowBlur => controls::MENU_SHADOW.1,
        N::ShadowAlpha => controls::MENU_SHADOW.2,
        N::FocusGrey => controls::MENU_FOCUS_GREY,
        N::InkAlpha => controls::MENU_INK_ALPHA,
        N::QuietAlpha => controls::MENU_QUIET_ALPHA,
        N::Font => controls::MENU_FONT,
        N::Line => controls::MENU_LINE,
        N::IconX => controls::MENU_ICON_X,
        N::Icon => controls::MENU_ICON,
        N::WordsX => controls::MENU_WORDS_X,
        N::ChoiceX => controls::MENU_CHOICE_X,
        N::Tick => controls::MENU_TICK,
        N::TickRight => controls::MENU_TICK_RIGHT,
    }
}

/// Plays at `rate` times the file's own speed (0.25 to 2), sound and picture.
pub(crate) fn preview_set_rate(rate: f64) {
    with_preview(|preview| preview.set_rate(rate));
}

/// The file the preview plays. Valid until the next call that lends text.
#[no_mangle]
pub extern "C" fn nr_preview_path() -> NrText {
    lend(with_preview(|preview| preview.path().to_owned()).unwrap_or_default())
}

/// Where a picture `width` by `height` sits on a screen it fills
/// (`player::video_controls::fullscreen_fit`). `out` takes its box: left, top, width, height.
///
/// # Safety
/// `out` must point to writable storage for 4 numbers.
#[no_mangle]
pub unsafe extern "C" fn nr_video_fullscreen_fit(
    width: f64,
    height: f64,
    screen_width: f64,
    screen_height: f64,
    out: *mut f64,
) {
    if out.is_null() {
        return;
    }
    let fitted = crate::preview::player::video_controls::fullscreen_fit(
        width,
        height,
        screen_width,
        screen_height,
    );
    // SAFETY: the caller passes storage for 4 numbers.
    unsafe { std::slice::from_raw_parts_mut(out, 4) }.copy_from_slice(&[
        fitted.x,
        fitted.y,
        fitted.width,
        fitted.height,
    ]);
}

/// The shapes of a glyph of the media controls (`glyph::MediaGlyph`), SVG elements in a
/// `glyph::MEDIA_BOX`-unit box; empty for a name that is none. Valid for as long as the app runs.
///
/// # Safety
/// `name` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_media_glyph_shapes(name: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let name = unsafe { text_of(name) };
    NrText::borrowing(
        crate::glyph::MediaGlyph::named(&name).map_or("", crate::glyph::MediaGlyph::shapes),
    )
}

/// The side of the box a media glyph's shapes are drawn in (`glyph::MEDIA_BOX`).
#[no_mangle]
pub extern "C" fn nr_media_glyph_box() -> u32 {
    crate::glyph::MEDIA_BOX
}

/// Whether the menu's Download asks where to save a copy of the open file. False in an
/// unattended run, which has nobody to answer; it logs what it would have done instead.
#[no_mangle]
pub extern "C" fn nr_preview_download_asks() -> bool {
    if !crate::launch::get().unattended {
        return true;
    }
    let path = with_preview(|preview| preview.path().to_owned()).unwrap_or_default();
    eprintln!("video menu: Download would ask where to save a copy of {path}");
    false
}

/// Saves a copy of the open file at `target` (`preview::copy_replacing`). Returns why not, or
/// an empty text. Valid until the next call that lends text.
///
/// # Safety
/// `target` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_preview_save_copy(target: *const c_char) -> NrText {
    // SAFETY: the caller passes a NUL-terminated string.
    let target = unsafe { text_of(target) };
    let Some(source) = with_preview(|preview| preview.path().to_owned()) else {
        return lend("no file is open".to_owned());
    };
    lend(
        crate::preview::copy_replacing(Path::new(&source), Path::new(&target))
            .err()
            .map_or_else(String::new, |error| error.to_string()),
    )
}

/// One player's controls (`player::video_controls::Controls`): over the open file's picture or
/// for its sound, or a page's player's, which rests while its block's player is not awake.
pub struct NrControls {
    controls: crate::preview::player::video_controls::Controls,
    /// The page player's block; `None` for the open file's controls.
    block: Option<String>,
    file_bytes: u64,
    /// The time line's parts as last handed out.
    lent: [String; 3],
    /// The scrub bar's preview's time as last handed out.
    preview_time: String,
}

/// Makes the controls of a picture or of a sound (`sound`) whose file at `path` is `file_bytes`
/// long, for the open file (`block` empty) or for a page's player, at `now` on the platform's
/// clock. A video's preview over its scrub bar takes the file's shape. Free them with
/// `nr_controls_free`.
///
/// # Safety
/// `block` and `path` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_new(
    sound: bool,
    file_bytes: u64,
    block: *const c_char,
    path: *const c_char,
    now: f64,
) -> *mut NrControls {
    use crate::preview::player::video_controls::{Controls, Style};
    // SAFETY: the caller passes NUL-terminated strings.
    let (block, path) = unsafe { (text_of(block), text_of(path)) };
    let mut controls = Controls::new(if sound { Style::Sound } else { Style::Video }, now);
    if !sound {
        if let Some((width, height)) = crate::media::probe_once(Path::new(&path)).dimensions {
            controls.set_shape(f64::from(width), f64::from(height));
        }
    }
    Box::into_raw(Box::new(NrControls {
        controls,
        block: (!block.is_empty()).then_some(block),
        file_bytes,
        lent: Default::default(),
        preview_time: String::new(),
    }))
}

/// Frees controls made by `nr_controls_new`.
///
/// # Safety
/// `controls` must be null or controls from `nr_controls_new`; they are not used again.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_free(controls: *mut NrControls) {
    if !controls.is_null() {
        // SAFETY: the caller gives up controls `nr_controls_new` made.
        drop(unsafe { Box::from_raw(controls) });
    }
}

/// What the controls show: their page player's state at rest, or else the open file's.
fn controls_playback(controls: &NrControls) -> crate::preview::player::video_controls::Playback {
    use crate::preview::player::video_controls::{Menu, Playback};
    if let Some(resting) = controls
        .block
        .as_deref()
        .and_then(|block| page_players().resting(block))
    {
        return resting;
    }
    let (info, menu) = with_preview(|preview| (preview.info(), preview.menu()))
        .unwrap_or((crate::preview::PreviewInfo::default(), Menu::Closed));
    Playback {
        playing: info.playing,
        position_seconds: info.position_seconds,
        duration_seconds: info.duration_seconds,
        has_audio: info.has_audio,
        muted: info.muted,
        volume: info.volume,
        fullscreen: info.fullscreen,
        has_played: info.has_played,
        menu,
        resting: false,
    }
}

fn controls_clock(now: f64) -> crate::preview::player::video_controls::Clock {
    let launch = crate::launch::get();
    crate::preview::player::video_controls::Clock {
        now,
        entrance: launch.held_ms(true),
        exit: launch.held_ms(false),
    }
}

/// The time line the controls write: its two parts over a picture ("0:03", "/ 0:05") and the
/// whole of it on a sound's pill.
#[repr(C)]
pub struct NrControlsClock {
    pub first: NrText,
    pub second: NrText,
    pub whole: NrText,
}

/// The controls' time line now. Valid until the next call of this for the same controls.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_clock(controls: *mut NrControls) -> NrControlsClock {
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &mut *controls };
    let shown = controls_playback(controls);
    let (first, second) =
        crate::preview::player::clock_parts(shown.position_seconds, shown.duration_seconds);
    let whole = crate::preview::player::clock_text(shown.position_seconds, shown.duration_seconds);
    controls.lent = [first, second, whole];
    NrControlsClock {
        first: NrText::borrowing(&controls.lent[0]),
        second: NrText::borrowing(&controls.lent[1]),
        whole: NrText::borrowing(&controls.lent[2]),
    }
}

/// The pointer came onto the controls `width` by `height`, or moved over them, to (`x`, `y`)
/// at `now`.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_pointer(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    now: f64,
) {
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &mut *controls };
    let shown = controls_playback(controls);
    controls
        .controls
        .pointer_moved(width, height, x, y, shown.has_audio, controls_clock(now));
}

/// The pointer left the controls at `now`.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_exit(controls: *mut NrControls, now: f64) {
    // SAFETY: the caller passes live controls.
    unsafe { &mut *controls }
        .controls
        .pointer_left(controls_clock(now));
}

/// A round ground the controls draw: rgb(32, 33, 36) at `strength`, when `shown`.
#[repr(C)]
pub struct NrControlsGround {
    pub shown: bool,
    pub rect: NrRect,
    pub strength: f64,
}

/// The controls over a picture as they are drawn now (`player::video_controls::VideoFrame`).
/// A glyph is its name (`nr_media_glyph_shapes`), empty for a button not drawn.
#[repr(C)]
pub struct NrVideoFrame {
    /// False once the panel has faded out: nothing is drawn.
    pub shown: bool,
    /// The panel with the shade, and the buttons inside it as a group of their own.
    pub panel: f64,
    pub buttons: f64,
    pub shade: NrRect,
    pub play: NrRect,
    pub play_glyph: NrText,
    pub mute: NrRect,
    pub mute_glyph: NrText,
    pub fullscreen: NrRect,
    pub fullscreen_glyph: NrText,
    pub overflow: NrRect,
    pub overflow_glyph: NrText,
    /// The ground the pointer left, fading out, then the one under it.
    pub left_ground: NrControlsGround,
    pub ground: NrControlsGround,
    /// The row the time's words are set in, and where each part starts when it is written.
    pub words: NrRect,
    pub first_shown: bool,
    pub first_x: f64,
    pub second_shown: bool,
    pub second_x: f64,
    /// The open volume control: its ground at `volume_strength`, and its slider's bar filled
    /// to `volume_level` while it shows.
    pub volume_shown: bool,
    pub volume_ground: NrRect,
    pub volume_strength: f64,
    pub slider_shown: bool,
    pub volume_bar: NrRect,
    pub volume_level: f64,
    /// The scrub bar: what is loaded, what has played, and whether its knob shows.
    pub track: NrRect,
    pub loaded: f64,
    pub played: f64,
    pub knob: bool,
    /// The menu open (`nr_video_menu`), and how many milliseconds since it opened, for its
    /// entrance (`nr_video_menu_entrance`).
    pub menu: NrMenuShown,
    pub menu_since_ms: f64,
    /// The preview over the scrub bar (`player::video_controls::scrub_preview`): the picture of
    /// the file `preview_seconds` in, and at its foot the time under the pointer, in a pill
    /// centred in `preview_line`.
    pub preview_shown: bool,
    pub preview_picture: NrRect,
    pub preview_line: NrRect,
    pub preview_time: NrText,
    pub preview_seconds: f64,
    /// How far apart the moments the preview shows are.
    pub preview_step_seconds: f64,
}

fn controls_ground(
    ground: Option<(crate::preview::player::video_controls::Rect, f64)>,
) -> NrControlsGround {
    let (rect, strength) = ground.unwrap_or_default();
    NrControlsGround {
        shown: ground.is_some(),
        rect: nr_rect(rect),
        strength,
    }
}

fn glyph_name(glyph: Option<crate::glyph::MediaGlyph>) -> NrText {
    NrText::borrowing(glyph.map_or("", crate::glyph::MediaGlyph::name))
}

/// The controls over a picture `width` by `height` at `now`, the time's two parts
/// (`nr_controls_clock`) `first_width` and `second_width` wide as the platform sets them. The
/// preview's time is valid until the next call of this for the same controls.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_video_frame(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    now: f64,
    first_width: f64,
    second_width: f64,
) -> NrVideoFrame {
    use crate::preview::player::video_controls::Rect;
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &mut *controls };
    let shown = controls_playback(controls);
    let frame = controls.controls.video_frame(
        width,
        height,
        &shown,
        controls.file_bytes,
        controls_clock(now),
        (first_width, second_width),
    );
    let shown = frame.is_some();
    let frame = frame.unwrap_or(crate::preview::player::video_controls::VideoFrame {
        panel: 0.0,
        buttons: 0.0,
        laid: crate::preview::player::video_controls::layout(width, height, false),
        play: None,
        mute: None,
        fullscreen: None,
        overflow: None,
        grounds: [None, None],
        time: (None, None),
        volume: None,
        loaded: 0.0,
        played: 0.0,
        knob: false,
        menu: NrMenuShown::Closed,
        menu_since_ms: 0.0,
        preview: None,
    });
    let laid = frame.laid;
    let preview = frame.preview;
    controls.preview_time = preview
        .map(|preview| crate::preview::player::format_timestamp(preview.at_seconds))
        .unwrap_or_default();
    let (volume_strength, slider) = frame.volume.unwrap_or((0.0, None));
    NrVideoFrame {
        shown,
        panel: frame.panel,
        buttons: frame.buttons,
        shade: nr_rect(laid.shade),
        play: nr_rect(laid.play),
        play_glyph: glyph_name(frame.play),
        mute: nr_rect(laid.mute),
        mute_glyph: glyph_name(frame.mute),
        fullscreen: nr_rect(laid.fullscreen),
        fullscreen_glyph: glyph_name(frame.fullscreen),
        overflow: nr_rect(laid.overflow),
        overflow_glyph: glyph_name(frame.overflow),
        left_ground: controls_ground(frame.grounds[0]),
        ground: controls_ground(frame.grounds[1]),
        words: nr_rect(Rect {
            x: laid.time_x,
            ..laid.row
        }),
        first_shown: frame.time.0.is_some(),
        first_x: frame.time.0.unwrap_or(0.0),
        second_shown: frame.time.1.is_some(),
        second_x: frame.time.1.unwrap_or(0.0),
        volume_shown: frame.volume.is_some(),
        volume_ground: nr_rect(laid.volume_ground),
        volume_strength,
        slider_shown: slider.is_some(),
        volume_bar: nr_rect(laid.volume_bar),
        volume_level: slider.unwrap_or(0.0),
        track: nr_rect(laid.track),
        loaded: frame.loaded,
        played: frame.played,
        knob: frame.knob,
        menu: frame.menu,
        menu_since_ms: frame.menu_since_ms,
        preview_shown: preview.is_some(),
        preview_picture: nr_rect(preview.map_or_else(Rect::default, |preview| preview.picture)),
        preview_line: nr_rect(preview.map_or_else(Rect::default, |preview| preview.line)),
        preview_time: NrText::borrowing(&controls.preview_time),
        preview_seconds: preview.map_or(0.0, |preview| preview.moment_seconds),
        preview_step_seconds: preview.map_or(0.0, |preview| preview.step_seconds),
    }
}

/// A sound's pill as it is drawn now (`player::video_controls::SoundFrame`): on its `ground`,
/// each button's glyph (`nr_media_glyph_shapes`) over the whole button in `ink`, empty for a
/// button not drawn; the time in `ink`; the bar in `track` with what has played in `ink`.
#[repr(C)]
pub struct NrSoundFrame {
    pub ground: crate::board::ffi::NrRgba,
    pub ink: crate::board::ffi::NrRgba,
    pub track_ink: crate::board::ffi::NrRgba,
    pub play: NrRect,
    pub play_glyph: NrText,
    /// The top-left of the time's 18 pt line.
    pub time_x: f64,
    pub time_y: f64,
    pub track: NrRect,
    pub played: f64,
    pub knob: bool,
    /// The menu's dots, and the speaker (when the file has sound), crossed out when muted.
    pub more: NrRect,
    pub more_glyph: NrText,
    pub mute: NrRect,
    pub mute_glyph: NrText,
}

/// A sound's pill `width` by `height`, its time line (`nr_controls_clock`) `clock_width` wide as
/// the platform sets it.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_sound_frame(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    clock_width: f64,
) -> NrSoundFrame {
    use crate::preview::player::sound_controls::{GROUND, INK, TRACK_ALPHA};
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &mut *controls };
    let shown = controls_playback(controls);
    let frame = controls
        .controls
        .sound_frame(width, height, &shown, clock_width);
    let colour = |[red, green, blue]: [f64; 3], alpha: f64| crate::board::ffi::NrRgba {
        red,
        green,
        blue,
        alpha,
    };
    NrSoundFrame {
        ground: colour(GROUND, 1.0),
        ink: colour(INK, 1.0),
        track_ink: colour(INK, TRACK_ALPHA),
        play: nr_rect(frame.laid.play),
        play_glyph: glyph_name(Some(frame.play)),
        time_x: frame.laid.time_x,
        time_y: frame.laid.time_y,
        track: nr_rect(frame.laid.track),
        played: frame.played,
        knob: frame.knob,
        more: nr_rect(frame.laid.more),
        more_glyph: glyph_name(Some(frame.more)),
        mute: nr_rect(frame.laid.mute),
        mute_glyph: glyph_name(frame.mute),
    }
}

/// What a press on the controls left for the shell.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrControlsPress {
    Done,
    /// Wake the page's player (`nr_page_player_wake`) and press again.
    Wake,
    /// Follow the drag with `nr_controls_drag` until the button is let go, then
    /// `nr_controls_release`.
    Drag,
    /// Ask where to save a copy of the file (the menu's Download).
    Download,
    /// Fill the screen with the picture or stop, as `nr_preview_info().fullscreen` says.
    Fullscreen,
}

/// A press at (`x`, `y`) on the controls `width` by `height` at `now`, the `clicks`th of a run
/// of clicks, the time line (`nr_controls_clock`) `clock_width` wide: the core does what it asks
/// (`player::video_controls::Press`) and returns what is left for the shell.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn nr_controls_press(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    clicks: u32,
    now: f64,
    clock_width: f64,
) -> NrControlsPress {
    use crate::preview::player::video_controls::{Chosen, FullscreenAsk, Press};
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &mut *controls };
    let shown = controls_playback(controls);
    let Some(press) = controls.controls.press(
        width,
        height,
        x,
        y,
        &shown,
        controls_clock(now),
        clock_width,
    ) else {
        return NrControlsPress::Done;
    };
    let fill = || {
        with_preview(|preview| preview.ask_fullscreen(FullscreenAsk::Toggle));
        NrControlsPress::Fullscreen
    };
    match press {
        Press::Wake => NrControlsPress::Wake,
        // An open menu takes every press: on a row it does what the row does, anywhere else it
        // only closes.
        Press::Menu => match with_preview(|preview| preview.press_menu(x, y, width, height)) {
            Some(Chosen::Download) => NrControlsPress::Download,
            _ => NrControlsPress::Done,
        },
        Press::ToggleMenu => {
            with_preview(Preview::toggle_menu);
            NrControlsPress::Done
        }
        Press::Fullscreen => fill(),
        Press::Play => {
            with_preview(Preview::toggle_playing);
            NrControlsPress::Done
        }
        Press::Mute => {
            with_preview(|preview| preview.set_muted(!shown.muted));
            NrControlsPress::Done
        }
        Press::Volume(_) => {
            controls_drag(controls, x);
            NrControlsPress::Drag
        }
        Press::Scrub(_) => {
            with_preview(Preview::begin_scrub);
            controls_drag(controls, x);
            NrControlsPress::Drag
        }
        Press::Picture => {
            with_preview(Preview::toggle_playing);
            if clicks == 2 {
                fill()
            } else {
                NrControlsPress::Done
            }
        }
    }
}

/// What a drag to `x` of what a press holds does: the volume slider sets the volume (a muted
/// file sounds again); the scrub bar moves the file to that share of its length.
fn controls_drag(controls: &mut NrControls, x: f64) {
    use crate::preview::player::video_controls::Drag;
    match controls.controls.drag(x) {
        Some(Drag::Volume(volume)) => {
            with_preview(|preview| {
                preview.set_muted(false);
                preview.set_volume(volume);
            });
        }
        Some(Drag::Seek(share)) => {
            with_preview(|preview| preview.seek_share(share));
        }
        None => {}
    }
}

/// The pointer moved to `x`, its button held since a press that answered 2.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_drag(controls: *mut NrControls, x: f64) {
    // SAFETY: the caller passes live controls.
    controls_drag(unsafe { &mut *controls }, x);
}

/// The button held since a press that answered 2 is let go: a drag on the scrub bar ends, a
/// file it paused playing again.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_release(controls: *mut NrControls) {
    use crate::preview::player::video_controls::Hold;
    // SAFETY: the caller passes live controls.
    if let Some(Hold::Scrub(_)) = unsafe { &mut *controls }.controls.release() {
        with_preview(Preview::end_scrub);
    }
}

/// What a key on the controls left for the shell.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrControlsKey {
    /// The key is not the player's: it goes on to whoever is next.
    PassOn,
    /// Wake the page's player and send the key again.
    Wake,
    /// The open menu took it.
    Menu,
    /// The player took it (its sound output and full screen may follow).
    Player,
    /// The menu's Download, as a press on it.
    Download,
}

/// A key on the controls `width` by `height`; one with ⌘, Control or ⌥ held is never the
/// player's.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_key(
    controls: *mut NrControls,
    press: NrKeyPress,
    width: f64,
    height: f64,
) -> NrControlsKey {
    use crate::preview::player::video_controls::{Chosen, KeyRoute};
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &*controls };
    let shown = controls_playback(controls);
    match controls.controls.route_key(
        crate::preview::player::Key::from_press(press.key),
        !press.modified(),
        &shown,
    ) {
        KeyRoute::Pass => NrControlsKey::PassOn,
        KeyRoute::Wake => NrControlsKey::Wake,
        KeyRoute::Menu(key) => {
            match with_preview(|preview| preview.menu_key(key, width, height)).flatten() {
                Some(Chosen::Download) => NrControlsKey::Download,
                Some(_) => NrControlsKey::Menu,
                None => NrControlsKey::PassOn,
            }
        }
        KeyRoute::Player(key) => {
            if with_preview(|preview| preview.key(key)).unwrap_or(false) {
                NrControlsKey::Player
            } else {
                NrControlsKey::PassOn
            }
        }
    }
}

/// Whether a press at (`x`, `y`) is the controls' to take: a page's player leaves a press on
/// none of its buttons and bars to the page, which selects the block (over a video, the picture
/// is the block's); the open file's controls take every press.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_takes_press(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
) -> bool {
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &*controls };
    controls.block.is_none()
        || controls
            .controls
            .operates(width, height, x, y, &controls_playback(controls))
}

/// A wheel travelled `down` (positive down) counted in `unit` (0 points, 3 clicks:
/// `scroll::route::wheel_points`) at (`x`, `y`) of the controls: over the open list of speeds it
/// scrolls the list. Returns whether it did.
///
/// # Safety
/// `controls` must be controls from `nr_controls_new`.
#[no_mangle]
pub unsafe extern "C" fn nr_controls_wheel(
    controls: *mut NrControls,
    width: f64,
    height: f64,
    x: f64,
    y: f64,
    down: f64,
    unit: u32,
) -> bool {
    // SAFETY: the caller passes live controls.
    let controls = unsafe { &*controls };
    if !controls
        .controls
        .wheel_scrolls(width, height, x, y, &controls_playback(controls))
    {
        return false;
    }
    let down = crate::scroll::route::wheel_points(down, unit);
    with_preview(|preview| preview.scroll_menu(down, width, height));
    true
}

/// Drops the list's rows `ids` on a folder row or the list's empty space (`Columns::list_drop`).
/// Returns why not, or an empty text.
///
/// # Safety
/// `ids` and `target` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_list_drop(ids: *const c_char, target: *const c_char) -> NrText {
    // SAFETY: the caller passes NUL-terminated strings.
    let (ids, target) = unsafe { (ids_of(&text_of(ids)), text_of(target)) };
    file_operation(|columns| {
        columns.list_drop(&ids, (!target.is_empty()).then_some(target.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::{nr_preview_close, nr_preview_open, with_preview};

    #[test]
    fn a_player_let_go_after_the_next_opened_leaves_the_next_ones_file() {
        crate::start();
        let path = std::ffi::CString::new("/nowhere/clip.mp4").expect("a path");
        // SAFETY: a NUL-terminated path.
        let first = unsafe { nr_preview_open(path.as_ptr()) };
        // SAFETY: a NUL-terminated path.
        let second = unsafe { nr_preview_open(path.as_ptr()) };
        assert!(
            !nr_preview_close(first),
            "the first file was already closed"
        );
        assert!(
            with_preview(|_| ()).is_some(),
            "the second file still plays"
        );
        assert!(nr_preview_close(second));
        assert!(with_preview(|_| ()).is_none());
    }
}
