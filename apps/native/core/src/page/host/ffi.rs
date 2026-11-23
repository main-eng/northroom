//! The page host's FFI (`page::host`). A shell creates one host per web view and passes it every
//! `nr` scheme request, every navigation and every message a page posts to `window.__nrPost`.
//! Returned text is borrowed and valid until the next host call that returns text on the same
//! thread, unless documented otherwise. A host may be called from any thread.

use super::{Body, Host, Problem, Reply};
use crate::ffi::{text_of, NrText};
use crate::Relock;
use std::ffi::c_char;
use std::path::Path;

/// The page host of one web view.
pub struct NrHost {
    host: Host,
    /// Text lent to the shell until the next lending call (`nr_host_draw_model`).
    lent: std::sync::Mutex<String>,
}

impl NrHost {
    fn opened(host: Host) -> *mut Self {
        Box::into_raw(Box::new(Self {
            host,
            lent: std::sync::Mutex::new(String::new()),
        }))
    }
}

/// One scheme response, kept until `nr_host_answer_free`.
pub struct NrHostAnswer {
    status: u16,
    headers: String,
    body: Vec<u8>,
    file: String,
    offset: u64,
    length: u64,
}

/// A response as the shell sends it. Its text and bytes are owned by the answer and valid until
/// it is freed.
#[repr(C)]
pub struct NrHostReply {
    pub status: u16,
    /// The headers as a JSON object of strings, Content-Length included.
    pub headers: NrText,
    /// The body, when the core holds it.
    pub body: *const u8,
    pub body_length: usize,
    /// Otherwise the file to send `length` bytes of, from `offset`; empty when `body` is set. The
    /// shell reads the file itself, so large media never crosses the FFI.
    pub file: NrText,
    pub offset: u64,
    pub length: u64,
}

pub const NR_HOST_STAGE_TIMEOUT: u32 = 1;
pub const NR_HOST_STAGE_FAILED: u32 = 2;
pub const NR_HOST_PROCESS_ENDED: u32 = 3;
pub const NR_HOST_NOT_LOADED: u32 = 4;
pub const NR_HOST_SHOW_FAILED: u32 = 5;
pub const NR_HOST_PAGE_FAILED: u32 = 8;
pub const NR_HOST_STILL_TIMEOUT: u32 = 9;
pub const NR_HOST_STILL_STOPPED: u32 = 10;
pub const NR_HOST_STILL_UNREADABLE: u32 = 11;
pub const NR_HOST_RULES_STORED: u32 = 12;
pub const NR_HOST_RULES_FAILED: u32 = 13;
pub const NR_HOST_NO_PICTURE: u32 = 14;
pub const NR_HOST_PICTURE_UNSIZED: u32 = 15;
pub const NR_HOST_TRANSPARENT_FAILED: u32 = 16;
pub const NR_HOST_SHEET_FAILED: u32 = 17;
pub const NR_HOST_RENDERER_UNREAD: u32 = 18;
pub const NR_HOST_EXPORT_CANCELLED: u32 = 19;
pub const NR_HOST_EXPORT_UNSTARTED: u32 = 20;
pub const NR_HOST_STEP_UNREAD: u32 = 21;
pub const NR_HOST_PNG_FAILED: u32 = 22;
pub const NR_HOST_FRAME_EARLY: u32 = 23;
pub const NR_HOST_ENCODER_MISSING: u32 = 24;
pub const NR_HOST_SOUND_ENCODER_MISSING: u32 = 25;
pub const NR_HOST_WRITER_UNSTARTED: u32 = 26;
pub const NR_HOST_ENCODER_STOPPED: u32 = 27;
pub const NR_HOST_NO_BUFFER: u32 = 28;
pub const NR_HOST_FRAME_UNENCODED: u32 = 29;
pub const NR_HOST_SOUND_UNPREPARED: u32 = 30;
pub const NR_HOST_SOUND_UNWRITTEN: u32 = 31;
pub const NR_HOST_UNFINISHED: u32 = 32;
pub const NR_HOST_SOUND_OUTPUT: u32 = 33;
pub const NR_HOST_NO_STEP: u32 = 34;
pub const NR_HOST_CORE_UNREAD: u32 = 35;
pub const NR_HOST_NOT_HERE: u32 = 36;

/// How a host's web view plays media and what its pages may open (`host::Kind::web`).
#[repr(C)]
pub struct NrHostWeb {
    /// A page may autoplay video.
    pub video_plays_alone: bool,
    /// A page may autoplay sound.
    pub sound_plays_alone: bool,
    /// A script may open a window without a user gesture.
    pub opens_windows: bool,
    /// An element may take the whole screen.
    pub fullscreen: bool,
    /// Media may be sent to another device.
    pub airplay: bool,
}

/// A line of the host's log (`nr_host_take_line`); `kind` is empty when there is none.
#[repr(C)]
pub struct NrHostLine {
    /// "blocked", "refused", "missing", "error" or "font". Static.
    pub kind: NrText,
    /// Valid until the next host function that hands out text on this thread.
    pub line: NrText,
}

thread_local! {
    static HOST_LENT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn lend_host(text: String) -> NrText {
    HOST_LENT.with(|lent| {
        let mut lent = lent.borrow_mut();
        *lent = text;
        NrText::borrowing(&lent)
    })
}

/// # Safety
/// `host` must be a live host from one of the `nr_host_open_*` functions.
unsafe fn host_of<'a>(host: *const NrHost) -> &'a Host {
    // SAFETY: as documented.
    &unsafe { &*host }.host
}

fn answer_box(reply: Reply) -> *mut NrHostAnswer {
    let headers: serde_json::Map<String, serde_json::Value> = reply
        .headers
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value.into()))
        .collect();
    let mut out = NrHostAnswer {
        status: reply.status,
        headers: serde_json::Value::Object(headers).to_string(),
        body: Vec::new(),
        file: String::new(),
        offset: 0,
        length: 0,
    };
    match reply.body {
        Body::Bytes(bytes) => out.body = bytes,
        Body::File {
            path,
            offset,
            length,
        } => {
            out.file = path.to_string_lossy().into_owned();
            out.offset = offset;
            out.length = length;
        }
    }
    Box::into_raw(Box::new(out))
}

/// Opens a host that runs the drive's HTML file `file` as a live page, serving its folder.
///
/// # Safety
/// `file` and `resources` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_host_open_page(
    file: *const c_char,
    resources: *const c_char,
) -> *mut NrHost {
    // SAFETY: as documented.
    let (file, resources) = unsafe { (text_of(file), text_of(resources)) };
    NrHost::opened(Host::page(Path::new(&file), Path::new(&resources)))
}

/// Opens a host for the page that renders 3D model stills one at a time (`Host::stills`). Each
/// model is passed with `nr_host_draw_model`.
///
/// # Safety
/// `resources` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_open_stills(resources: *const c_char) -> *mut NrHost {
    // SAFETY: as documented.
    let resources = unsafe { text_of(resources) };
    NrHost::opened(Host::stills(Path::new(&resources)))
}

/// A model passed to a still host: the JavaScript call that makes its page render it (lent until
/// the next `nr_host_draw_model` on that host), and its turn, which the page's reply names.
#[repr(C)]
pub struct NrStillTurn {
    pub call: NrText,
    pub turn: u64,
}

/// Passes a still host the model `file` to render next (`Host::draw_model`).
///
/// # Safety
/// `host` must be a live host from `nr_host_open_stills`, and `file` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_draw_model(
    host: *const NrHost,
    file: *const c_char,
) -> NrStillTurn {
    // SAFETY: as documented.
    let (opened, file) = unsafe { (&*host, text_of(file)) };
    let (call, turn) = opened.host.draw_model(Path::new(&file));
    let mut lent = opened.lent.relock();
    *lent = call;
    NrStillTurn {
        call: NrText::borrowing(&lent),
        turn,
    }
}

/// # Safety
/// `host` must be null or a host from `nr_host_open_*`; it is not used again.
#[no_mangle]
pub unsafe extern "C" fn nr_host_close(host: *mut NrHost) {
    if !host.is_null() {
        // SAFETY: the caller hands back ownership of a host from `nr_host_open_*`.
        drop(unsafe { Box::from_raw(host) });
    }
}

/// How the shell configures the host's web view: what autoplays and what a page may open.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_web(host: *const NrHost) -> NrHostWeb {
    // SAFETY: as documented.
    let web = unsafe { host_of(host) }.kind().web();
    NrHostWeb {
        video_plays_alone: web.video_plays_alone,
        sound_plays_alone: web.sound_plays_alone,
        opens_windows: web.opens_windows,
        fullscreen: web.fullscreen,
        airplay: web.airplay,
    }
}

/// Called when the host's web content process dies. Returns true when the shell should reload
/// the page itself (a live HTML page). A stage's owner is notified instead, and a still fails.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_crash_reloads(host: *const NrHost) -> bool {
    // SAFETY: as documented.
    unsafe { host_of(host) }.kind().reloads_on_crash()
}

/// The side, in CSS pixels, of the square web view a 3D model's still is rendered in.
#[no_mangle]
pub extern "C" fn nr_host_still_stage() -> u32 {
    super::STILL_STAGE
}

/// The URL the web view loads first. Valid for the host's lifetime.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_entry(host: *const NrHost) -> NrText {
    // SAFETY: as documented.
    let entry = unsafe { host_of(host) }.entry();
    NrText::borrowing(entry)
}

/// Answers a request for `url` with its `Range` header (empty for none). It reads files and
/// rewrites pages, so call it off the UI thread. Free the result with `nr_host_answer_free`.
///
/// # Safety
/// `host` must be a live host; `url` and `range` NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_host_answer(
    host: *const NrHost,
    url: *const c_char,
    range: *const c_char,
) -> *mut NrHostAnswer {
    // SAFETY: as documented.
    let (host, url, range) = unsafe { (host_of(host), text_of(url), text_of(range)) };
    answer_box(host.answer(&url, Some(range.as_str()).filter(|range| !range.is_empty())))
}

/// # Safety
/// `answer` must be a live answer from `nr_host_answer`.
#[no_mangle]
pub unsafe extern "C" fn nr_host_reply(answer: *const NrHostAnswer) -> NrHostReply {
    // SAFETY: as documented.
    let answer = unsafe { &*answer };
    NrHostReply {
        status: answer.status,
        headers: NrText::borrowing(&answer.headers),
        body: answer.body.as_ptr(),
        body_length: answer.body.len(),
        file: NrText::borrowing(&answer.file),
        offset: answer.offset,
        length: answer.length,
    }
}

/// # Safety
/// `answer` must be null or an answer from the host; it is not used again.
#[no_mangle]
pub unsafe extern "C" fn nr_host_answer_free(answer: *mut NrHostAnswer) {
    if !answer.is_null() {
        // SAFETY: the caller hands back ownership of an answer the host made.
        drop(unsafe { Box::from_raw(answer) });
    }
}

/// Whether the web view may navigate to `url` (empty when absent). `main_frame` is false for a
/// subframe, and `download` is true when the navigation would download. Refusals are logged.
///
/// # Safety
/// `host` must be a live host; `url` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_navigate(
    host: *const NrHost,
    url: *const c_char,
    main_frame: bool,
    download: bool,
) -> bool {
    // SAFETY: as documented.
    let (host, url) = unsafe { (host_of(host), text_of(url)) };
    host.navigate(&url, main_frame, download)
}

/// Handles a JSON message a page posted to `window.__nrPost`, logging what it reports. Returns
/// the replaced page when the message names one, else empty.
///
/// # Safety
/// `host` must be a live host; `message` a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_page_message(
    host: *const NrHost,
    message: *const c_char,
) -> NrText {
    // SAFETY: as documented.
    let (host, message) = unsafe { (host_of(host), text_of(message)) };
    lend_host(host.page_message(&message).unwrap_or_default())
}

/// The log as JSON: `{ blocked, refused, missing, errors, fonts: [line], requests,
/// rangeRequests, served: [path] }`.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_log(host: *const NrHost) -> NrText {
    // SAFETY: as documented.
    lend_host(unsafe { host_of(host) }.log_json().to_string())
}

/// Takes the oldest new log line. Call until `kind` is empty.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_take_line(host: *const NrHost) -> NrHostLine {
    // SAFETY: as documented.
    match unsafe { host_of(host) }.take_line() {
        Some((line, text)) => NrHostLine {
            kind: NrText::borrowing(line.name()),
            line: lend_host(text),
        },
        None => NrHostLine {
            kind: NrText::borrowing(""),
            line: NrText::borrowing(""),
        },
    }
}

/// Clears the log lines, keeping the counts and the served list.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_clear_log(host: *const NrHost) {
    // SAFETY: as documented.
    unsafe { host_of(host) }.clear_log();
}

/// The script injected into a live page at document start. It posts errors to the host and
/// provides `window.__nr.sql` for querying a database in the page's folder. Static.
#[no_mangle]
pub extern "C" fn nr_host_page_errors_script() -> NrText {
    NrText::borrowing(super::PAGE_SCRIPT)
}

/// Whether a database the page queried was changed by something other than the page since its
/// last query (`Host::data_changed`). Each change is reported once; the shell then evaluates
/// `nr_host_data_changed_call` in the page.
///
/// # Safety
/// `host` must be a live host.
#[no_mangle]
pub unsafe extern "C" fn nr_host_data_changed(host: *const NrHost) -> bool {
    // SAFETY: as documented.
    unsafe { host_of(host) }.data_changed()
}

/// The JavaScript expression that tells a page its database changed. Static.
#[no_mangle]
pub extern "C" fn nr_host_data_changed_call() -> NrText {
    NrText::borrowing(super::DATA_CHANGED_CALL)
}

/// The message for a host problem code (`NR_HOST_*`), with `detail` (the platform's error, a
/// folder, a codec, or unparseable text) where relevant, and its numbers: an image's `first` ×
/// `second` pixels (`NR_HOST_PICTURE_UNSIZED`), or frame `first` (`NR_HOST_PNG_FAILED`,
/// `NR_HOST_FRAME_EARLY`, `NR_HOST_FRAME_UNENCODED`). Empty for an unknown code.
///
/// # Safety
/// `detail` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_problem(
    problem: u32,
    detail: *const c_char,
    first: u64,
    second: u64,
) -> NrText {
    // SAFETY: as documented.
    let detail = unsafe { text_of(detail) };
    let problem = match problem {
        NR_HOST_STAGE_TIMEOUT => Problem::StageTimeout,
        NR_HOST_STAGE_FAILED => Problem::StageFailed,
        NR_HOST_PROCESS_ENDED => Problem::ProcessEnded,
        NR_HOST_NOT_LOADED => Problem::NotLoaded,
        NR_HOST_SHOW_FAILED => Problem::ShowFailed,
        NR_HOST_PAGE_FAILED => Problem::PageFailed,
        NR_HOST_STILL_TIMEOUT => Problem::StillTimeout,
        NR_HOST_STILL_STOPPED => Problem::StillStopped,
        NR_HOST_STILL_UNREADABLE => Problem::StillUnreadable,
        NR_HOST_RULES_STORED => Problem::RulesStored,
        NR_HOST_RULES_FAILED => Problem::RulesFailed,
        NR_HOST_NO_PICTURE => Problem::NoPicture,
        NR_HOST_PICTURE_UNSIZED => Problem::PictureUnsized {
            width: first,
            height: second,
        },
        NR_HOST_TRANSPARENT_FAILED => Problem::TransparentFailed,
        NR_HOST_SHEET_FAILED => Problem::SheetFailed,
        NR_HOST_RENDERER_UNREAD => Problem::RendererUnread,
        NR_HOST_EXPORT_CANCELLED => Problem::ExportCancelled,
        NR_HOST_EXPORT_UNSTARTED => Problem::ExportUnstarted,
        NR_HOST_STEP_UNREAD => Problem::StepUnread,
        NR_HOST_PNG_FAILED => Problem::PngFailed { frame: first },
        NR_HOST_FRAME_EARLY => Problem::FrameEarly { frame: first },
        NR_HOST_ENCODER_MISSING => Problem::EncoderMissing,
        NR_HOST_SOUND_ENCODER_MISSING => Problem::SoundEncoderMissing,
        NR_HOST_WRITER_UNSTARTED => Problem::WriterUnstarted,
        NR_HOST_ENCODER_STOPPED => Problem::EncoderStopped,
        NR_HOST_NO_BUFFER => Problem::NoBuffer,
        NR_HOST_FRAME_UNENCODED => Problem::FrameUnencoded { frame: first },
        NR_HOST_SOUND_UNPREPARED => Problem::SoundUnprepared,
        NR_HOST_SOUND_UNWRITTEN => Problem::SoundUnwritten,
        NR_HOST_UNFINISHED => Problem::Unfinished,
        NR_HOST_SOUND_OUTPUT => Problem::SoundOutput,
        NR_HOST_NO_STEP => Problem::NoStep,
        NR_HOST_CORE_UNREAD => Problem::CoreUnread,
        NR_HOST_NOT_HERE => Problem::NotHere,
        _ => return lend_host(String::new()),
    };
    lend_host(problem.text(&detail))
}

/// Whether `NR_HOST_DEBUG` is set, so the host traces to stderr (`nr_host_trace`).
#[no_mangle]
pub extern "C" fn nr_host_tracing() -> bool {
    super::tracing()
}

/// Writes `text` to stderr as a trace line, timed from the first line, when `nr_host_tracing`.
///
/// # Safety
/// `text` must be a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_host_trace(text: *const c_char) {
    static FIRST: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    if !super::tracing() {
        return;
    }
    let first = *FIRST.get_or_init(std::time::Instant::now);
    // SAFETY: as documented.
    let line = super::trace_line(first.elapsed().as_secs_f64(), &unsafe { text_of(text) });
    use std::io::Write as _;
    let _ = std::io::stderr().write_all(line.as_bytes());
}

/// Where the page host keeps its compiled network block list (`paths::Paths::scene_host`).
#[no_mangle]
pub extern "C" fn nr_host_rules_folder() -> NrText {
    lend_host(
        crate::paths::get()
            .scene_host
            .to_string_lossy()
            .into_owned(),
    )
}

/// The block list that stops every request from leaving the machine (WebKit content rule
/// JSON). Static.
#[no_mangle]
pub extern "C" fn nr_host_block_rules() -> NrText {
    NrText::borrowing(super::BLOCK_RULES)
}

/// The identifier the compiled block list is stored under. Static.
#[no_mangle]
pub extern "C" fn nr_host_block_rules_name() -> NrText {
    NrText::borrowing(super::BLOCK_RULES_NAME)
}

/// The scheme the host serves. Static.
#[no_mangle]
pub extern "C" fn nr_host_scheme() -> NrText {
    NrText::borrowing(super::SCHEME)
}

/// How long a page has to report ready, unless the shell sets another timeout.
#[no_mangle]
pub extern "C" fn nr_host_ready_timeout_seconds() -> f64 {
    super::READY_TIMEOUT_SECONDS
}

/// How long the stage, or a model's still, has to load.
#[no_mangle]
pub extern "C" fn nr_host_start_timeout_seconds() -> f64 {
    super::START_TIMEOUT_SECONDS
}

/// Recovers a transparent image into `out` from renders of it over black and over white.
/// Buffers are premultiplied RGBA, `length` bytes each.
///
/// # Safety
/// `black`, `white` and `out` must each hold `length` bytes.
#[no_mangle]
pub unsafe extern "C" fn nr_pixels_unground(
    black: *const u8,
    white: *const u8,
    out: *mut u8,
    length: usize,
) {
    // SAFETY: as documented.
    let (black, white, out) = unsafe {
        (
            std::slice::from_raw_parts(black, length),
            std::slice::from_raw_parts(white, length),
            std::slice::from_raw_parts_mut(out, length),
        )
    };
    super::unground(black, white, out);
}
