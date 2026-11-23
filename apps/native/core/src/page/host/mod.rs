//! The page host, shared by every shell: how the `nr://files/…` scheme responds, the scripts
//! and documents a web view gets, which navigations are allowed, and what is logged. A shell
//! creates the web view, injects what this provides, passes each request, navigation and page
//! message here, and returns the result. The shell holds none of these rules. There are two
//! kinds of web view: a drive HTML file running as a live page, and the page that renders 3D
//! model stills.
//!
//! The web view scripts live in `core/assets/host`: `page-errors.js` (for live pages),
//! `page-data.js` (`window.__nr.sql`, for querying a database in the page's folder) and
//! `model-still.html` with `model-still.js` (3D model stills). Each shell defines one bridge,
//! `window.__nrPost(message)`, that passes a message object to the host as JSON
//! (`page_message`).

pub mod ffi;

use crate::sheets::database::{self, Access, Version};
use crate::Relock;

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

/// The scheme, and single origin, that serves every file a page reads.
pub const SCHEME: &str = "nr";
/// The host name in every scheme URL.
const HOST: &str = "files";
pub const ORIGIN: &str = "nr://files";
/// Pinned library versions, served from the shell resources' `vendor/` folder.
pub const GSAP_VERSION: &str = "3.15.0";
pub const THREE_VERSION: &str = "0.186.0";
/// How long a page has to report ready, unless the shell sets another timeout.
pub const READY_TIMEOUT_SECONDS: f64 = 20.0;
/// How long a model's still has to load.
pub const START_TIMEOUT_SECONDS: f64 = 30.0;

/// The web engine block list (WebKit content rule format) that stops every request leaving the
/// machine: http, https, WebSocket and FTP. Its name keys the shell's compiled copy, so change
/// it whenever the rules change.
pub const BLOCK_RULES_NAME: &str = "northroom-no-network-1";
pub const BLOCK_RULES: &str = r#"[{"trigger":{"url-filter":"^https?://"},"action":{"type":"block"}},{"trigger":{"url-filter":"^wss?://"},"action":{"type":"block"}},{"trigger":{"url-filter":"^ftp://"},"action":{"type":"block"}}]"#;

/// Where a host with its own document (a still host) serves it.
const DOCUMENT_PATH: &str = "__nr/stage.html";
/// Where a still host serves its current model, `__nr/model/<turn>/<file>`, with the model's
/// folder alongside (`Host::draw_model`).
const MODEL_PATH: &str = "__nr/model/";
/// Where a page's database queries are answered (`Host::ask`).
const DATABASE_PATH: &str = "__nr/sql";
const JSON: &str = "application/json; charset=utf-8";
const HTML: &str = "text/html; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

/// The live page script that reports script errors.
pub const PAGE_ERRORS: &str = include_str!("../../../assets/host/page-errors.js");
/// The live page script that provides `window.__nr.sql` for querying databases in its folder.
pub const PAGE_DATA: &str = include_str!("../../../assets/host/page-data.js");
/// The script injected into a live page at document start.
pub const PAGE_SCRIPT: &str = concat!(
    include_str!("../../../assets/host/page-errors.js"),
    "\n",
    include_str!("../../../assets/host/page-data.js")
);
/// What a shell evaluates in a live page when something else changed a database it queried
/// (`Host::data_changed`). It dispatches `nr:change` on the window.
pub const DATA_CHANGED_CALL: &str = "window.dispatchEvent(new Event('nr:change')); undefined";
const STILL_SCRIPT: &str = include_str!("../../../assets/host/model-still.js");
const STILL_DOCUMENT: &str = include_str!("../../../assets/host/model-still.html");

fn vendor() -> String {
    format!("{ORIGIN}/__nr/vendor")
}

fn three() -> String {
    format!("{}/three-{THREE_VERSION}", vendor())
}

fn gsap() -> String {
    format!("{}/gsap-{GSAP_VERSION}", vendor())
}

/// `template` with each `{{name}}` replaced in one pass, so a value is never expanded again. A
/// JavaScript slot is written as `/*{{name}}*/`, which survives formatting, and the whole
/// comment is replaced. The result drops the template file's trailing newline.
fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let template = template.trim_end_matches('\n');
    let mut out = String::with_capacity(
        template.len() + values.iter().map(|(_, value)| value.len()).sum::<usize>(),
    );
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let known = after.find("}}").and_then(|close| {
            values
                .iter()
                .find(|(name, _)| *name == &after[..close])
                .map(|(_, value)| (close, *value))
        });
        match known {
            Some((close, value)) => {
                let script_comment =
                    rest[..open].ends_with("/*") && after[close + 2..].starts_with("*/");
                let closing = if script_comment {
                    out.truncate(out.len() - 2);
                    "}}*/".len()
                } else {
                    "}}".len()
                };
                out.push_str(value);
                rest = &after[close + closing..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The side of the square web view a 3D model still renders in, in CSS pixels; `SIZE` in
/// `model-still.js` must match.
pub const STILL_STAGE: u32 = 1024;

/// The page that renders 3D model stills one at a time on request (`model-still.js`).
pub fn model_still_page() -> String {
    fill(
        STILL_DOCUMENT,
        &[
            ("three", &three()),
            ("script", STILL_SCRIPT.trim_end_matches('\n')),
        ],
    )
}

/// The JavaScript call that makes a still page render the model at `address` for `turn`. The
/// page posts back `{ turn, png }` or `{ turn, error }`.
pub fn draw_model_call(address: &str, turn: u64) -> String {
    format!(
        "window.__nrDrawStill({}, {turn})",
        Value::String(address.to_owned())
    )
}

/// A file's scheme URL from its path in the served folder.
pub fn address_of(relative: &str) -> String {
    format!("{ORIGIN}/{}", encode_path(relative))
}

/// Percent-encodes everything not allowed in a URL path: every byte except ASCII letters,
/// digits and `!$&'()*+,-./:;=@_~`.
fn encode_path(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"!$&'()*+,-./:;=@_~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    char::from(byte)
        .to_digit(16)
        .and_then(|digit| u8::try_from(digit).ok())
}

/// Percent-decodes a URL path leniently: an invalid `%` escape stays literal, and a path whose
/// bytes are not UTF-8 stays as written.
fn decode_path(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        out.push(byte);
        index += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_owned())
}

/// Strict percent-decoding: `None` for an invalid `%` escape or non-UTF-8 bytes.
fn decode_strict(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(*bytes.get(index + 1)?)?;
            let low = hex(*bytes.get(index + 2)?)?;
            out.push(high * 16 + low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

const THIS_COMPUTER: &str = if cfg!(target_os = "macos") {
    "this Mac"
} else {
    "this computer"
};

/// Host problem codes and their messages, covering the page host, its snapshots, and the
/// export writer and encoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    StageTimeout,
    StageFailed,
    ProcessEnded,
    NotLoaded,
    ShowFailed,
    NoAnswer,
    /// The web view produced no animation frames in time. A web view in a covered or off-screen
    /// window (document hidden) gets none by design; the picture renders once visible.
    Unsettled,
    PageFailed,
    StillTimeout,
    StillStopped,
    StillUnreadable,
    RulesStored,
    RulesFailed,
    /// The web view's snapshot came back empty.
    NoPicture,
    /// A snapshot could not be redrawn at the requested size.
    PictureUnsized {
        width: u64,
        height: u64,
    },
    TransparentFailed,
    SheetFailed,
    /// The core renderer's output could not be parsed (`detail`).
    RendererUnread,
    ExportCancelled,
    ExportUnstarted,
    /// The core export's output could not be parsed (`detail`).
    StepUnread,
    PngFailed {
        frame: u64,
    },
    FrameEarly {
        frame: u64,
    },
    /// The system has no encoder for the codec `detail`.
    EncoderMissing,
    SoundEncoderMissing,
    WriterUnstarted,
    EncoderStopped,
    NoBuffer,
    FrameUnencoded {
        frame: u64,
    },
    SoundUnprepared,
    SoundUnwritten,
    Unfinished,
    /// A preview's sound output would not start.
    SoundOutput,
    /// The shell received a step it does not support (`detail`).
    NoStep,
    /// The core's output could not be parsed (`detail`).
    CoreUnread,
    /// The platform has no page host yet. Non-Mac shells return this for every call, as the core's
    /// media does there.
    NotHere,
}

/// The platform's error text, or "unknown" when it gave none.
fn account(detail: &str) -> &str {
    if detail.is_empty() {
        "unknown"
    } else {
        detail
    }
}

impl Problem {
    /// The message, with `detail` (the platform's error, a folder, a codec, or unparseable text)
    /// where relevant.
    pub fn text(self, detail: &str) -> String {
        match self {
            Self::StageTimeout => format!("the stage did not load in {START_TIMEOUT_SECONDS} s"),
            Self::StageFailed => format!("the stage did not load: {detail}"),
            Self::ProcessEnded => "the web content process ended".to_owned(),
            Self::NotLoaded => "the stage is not loaded".to_owned(),
            Self::ShowFailed => format!("the stage could not show the frame: {detail}"),
            Self::NoAnswer => "the stage gave no answer".to_owned(),
            Self::Unsettled => {
                "the web view drew no new frame in time; the picture may be stale".to_owned()
            }
            Self::PageFailed => format!("the page did not load: {detail}"),
            Self::StillTimeout => format!("the model was not drawn in {START_TIMEOUT_SECONDS} s"),
            Self::StillStopped => "the web view stopped".to_owned(),
            Self::StillUnreadable => "the still could not be read".to_owned(),
            Self::RulesStored => format!("the network block list could not be stored in {detail}"),
            Self::RulesFailed => {
                format!(
                    "the network block list could not be made: {}",
                    account(detail)
                )
            }
            Self::NoPicture => format!("the web view gave no picture: {}", account(detail)),
            Self::PictureUnsized { width, height } => {
                format!("the picture could not be drawn at {width}×{height}")
            }
            Self::TransparentFailed => "the transparent picture could not be made".to_owned(),
            Self::SheetFailed => "the sheet could not be drawn".to_owned(),
            Self::RendererUnread => format!("the renderer's answer could not be read: {detail}"),
            Self::ExportCancelled => "the export was cancelled; no file was left".to_owned(),
            Self::ExportUnstarted => {
                "the export asked for a picture before it started its page host".to_owned()
            }
            Self::StepUnread => format!("the export's next step could not be read: {detail}"),
            Self::PngFailed { frame } => format!("frame {frame} could not be encoded as PNG"),
            Self::FrameEarly { frame } => format!("frame {frame} came before the movie was open"),
            Self::EncoderMissing => {
                format!("the {detail} encoder is not available on {THIS_COMPUTER}")
            }
            Self::SoundEncoderMissing => {
                format!("the sound encoder is not available on {THIS_COMPUTER}")
            }
            Self::WriterUnstarted => format!("the writer did not start: {}", account(detail)),
            Self::EncoderStopped => format!("the encoder stopped: {}", account(detail)),
            Self::NoBuffer => "the encoder gave no frame to draw into".to_owned(),
            Self::FrameUnencoded { frame } => {
                format!("frame {frame} could not be encoded: {}", account(detail))
            }
            Self::SoundUnprepared => "the sound could not be prepared".to_owned(),
            Self::SoundUnwritten => format!("the sound could not be written: {}", account(detail)),
            Self::Unfinished => format!("the file could not be finished: {}", account(detail)),
            Self::SoundOutput => format!("a player's sound output did not start: {detail}"),
            Self::NoStep => format!("no step {detail}"),
            Self::CoreUnread => format!("the core's answer could not be read: {detail}"),
            Self::NotHere => "scene pages are not shown on this platform yet".to_owned(),
        }
    }
}

// Tracing.

/// Whether `NR_HOST_DEBUG` is set (to anything), which makes the host trace to stderr.
pub fn tracing() -> bool {
    std::env::var_os("NR_HOST_DEBUG").is_some()
}

/// One line of the host's trace: `host <seconds since the first line>: <text>`.
pub fn trace_line(seconds: f64, text: &str) -> String {
    format!("host {seconds:.3}: {text}\n")
}

// Pixels.

/// Recovers a transparent image from renders over black and over white (premultiplied RGBA
/// rows). Over black each pixel is its colour times alpha; over white it adds (1 - alpha) of
/// white, so alpha is 255 minus the difference. `out` has the same length as both. This gets a
/// transparent image from a web engine that only renders over an opaque background.
pub fn unground(black: &[u8], white: &[u8], out: &mut [u8]) {
    for ((dark, light), pixel) in black
        .as_chunks::<4>()
        .0
        .iter()
        .zip(white.as_chunks::<4>().0)
        .zip(out.as_chunks_mut::<4>().0)
    {
        let (r, g, b) = (i32::from(dark[0]), i32::from(dark[1]), i32::from(dark[2]));
        // Each channel shows how much white came through; take their rounded mean.
        let through =
            (i32::from(light[0]) - r) + (i32::from(light[1]) - g) + (i32::from(light[2]) - b);
        let alpha = 255 - (f64::from(through) / 3.0).round() as i32;
        // A premultiplied colour is never above its alpha.
        let alpha = alpha.max(r).max(g).max(b).min(255);
        pixel.copy_from_slice(&[dark[0], dark[1], dark[2], alpha as u8]);
    }
}

// Fonts.

/// A face of a family in a fonts folder, as named by the stylesheet a Google Fonts link
/// represents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Face {
    pub file: String,
    /// One weight, or `None` for every weight (a variable face).
    pub weight: Option<u32>,
    pub italic: bool,
    /// The CSS `format()`.
    pub format: &'static str,
}

impl Face {
    /// The `@font-face` rule that serves this face as `family`.
    fn rule(&self, family: &str) -> String {
        let weight = self
            .weight
            .map_or_else(|| "100 900".to_owned(), |weight| weight.to_string());
        let style = if self.italic { "italic" } else { "normal" };
        format!(
            "@font-face{{font-family:'{family}';src:url('{ORIGIN}/__nr/fonts/{}') format('{}');font-weight:{weight};font-style:{style};font-display:block}}\n",
            encode_path(&self.file),
            self.format
        )
    }
}

/// The families a Google Fonts link's `query` requests, without their styles.
fn asked_families(query: &str) -> Vec<String> {
    let mut families = Vec::new();
    for item in query.split('&').filter(|item| !item.is_empty()) {
        let Some((key, value)) = item.trim_start_matches('=').split_once('=') else {
            continue;
        };
        if key != "family" || value.is_empty() {
            continue;
        }
        let value = decode_strict(&value.replace('+', " ")).unwrap_or_else(|| value.to_owned());
        // The v1 API lists families in one value, separated by "|".
        for family in value.split('|').filter(|family| !family.is_empty()) {
            let name = family
                .split(':')
                .find(|part| !part.is_empty())
                .unwrap_or_default();
            families.push(name.trim_matches([' ', '\t']).to_owned());
        }
    }
    families
}

const WEIGHTS: [(&str, u32); 14] = [
    ("extralight", 200),
    ("ultralight", 200),
    ("extrabold", 800),
    ("ultrabold", 800),
    ("semibold", 600),
    ("demibold", 600),
    ("thin", 100),
    ("light", 300),
    ("regular", 400),
    ("book", 400),
    ("medium", 500),
    ("bold", 700),
    ("black", 900),
    ("heavy", 900),
];
const ITALIC: [&str; 2] = ["italic", "oblique"];
const FILLER: [&str; 7] = [
    "variablefont",
    "variable",
    "wght",
    "opsz",
    "latin",
    "ext",
    "vf",
];

/// A name normalized for font matching: lower case, letters and digits only.
fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect()
}

/// The faces of `family` in `folder`, by file name: the family name followed only by style
/// words. Anything else after the family means another family ("Inter" is not "Inter Tight").
pub fn faces(folder: &Path, family: &str) -> Vec<Face> {
    let wanted = plain(family);
    if wanted.is_empty() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    // A `._` file is another file's AppleDouble metadata, not a face.
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.starts_with("._"))
        .collect();
    names.sort();
    names
        .iter()
        .filter_map(|name| face(name, &wanted))
        .collect()
}

fn face(name: &str, wanted: &str) -> Option<Face> {
    let path = Path::new(name);
    let format = match path.extension()?.to_str()?.to_lowercase().as_str() {
        "woff2" => "woff2",
        "woff" => "woff",
        "ttf" => "truetype",
        "otf" => "opentype",
        _ => return None,
    };
    let mut rest = plain(path.file_stem()?.to_str()?)
        .strip_prefix(wanted)?
        .to_owned();
    let mut italic = false;
    for word in ITALIC {
        if rest.contains(word) {
            italic = true;
            rest = rest.replace(word, "");
        }
    }
    // One weight word: the first from the list that the name contains.
    let mut weight = WEIGHTS
        .iter()
        .find(|(word, _)| rest.contains(word))
        .map(|&(word, value)| {
            rest = rest.replace(word, "");
            value
        });
    for word in FILLER {
        rest = rest.replace(word, "");
    }
    if !rest.chars().all(char::is_numeric) {
        return None;
    }
    if weight.is_none() {
        weight = rest
            .parse::<u32>()
            .ok()
            .filter(|number| (1..=1000).contains(number));
    }
    Some(Face {
        file: name.to_owned(),
        weight,
        italic,
        format,
    })
}

// The log.

/// Log line kinds, for what a page requested and did not get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A blocked network attempt: "fetch https://example.com/ping (scenes/a.html)".
    Blocked,
    /// A refused request for a file outside the served folder.
    Refused,
    /// A missing file inside the folder.
    Missing,
    /// A page-reported error: a script error, a file that failed to load, a late frame.
    Error,
    /// A Google Fonts family a page requested that is neither packaged nor in `fonts/`.
    Font,
}

impl Line {
    const ALL: [Line; 5] = [
        Line::Blocked,
        Line::Refused,
        Line::Missing,
        Line::Error,
        Line::Font,
    ];

    /// The kind's name for the shell.
    pub fn name(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Refused => "refused",
            Self::Missing => "missing",
            Self::Error => "error",
            Self::Font => "font",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Default)]
struct Log {
    /// Each kind's unique lines, in arrival order.
    lines: [Vec<String>; 5],
    /// Lines added since the shell last took them.
    fresh: Vec<(Line, String)>,
    requests: u64,
    range_requests: u64,
    /// Every served file, by its path in the folder.
    served: BTreeSet<String>,
    /// The databases the page queried, each with its version after the page's last query.
    asked: BTreeMap<PathBuf, Version>,
    /// Something else changed a queried database, and the page has not been told yet.
    asked_changed: bool,
}

// The scheme.

/// The document a host serves at its entry, which also sets its navigation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A drive HTML file running as a live page; links within its folder open.
    Page,
    /// A 3D model still renderer.
    Still,
}

/// The web view's media and window policy, as the shell configures it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Web {
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

impl Kind {
    /// A live page follows browser autoplay rules (video yes, sound no); a still plays nothing.
    /// Neither may open windows, go full screen or send media elsewhere.
    pub fn web(self) -> Web {
        let plays = |video, sound| Web {
            video_plays_alone: video,
            sound_plays_alone: sound,
            opens_windows: false,
            fullscreen: false,
            airplay: false,
        };
        match self {
            Self::Page => plays(true, false),
            Self::Still => plays(false, false),
        }
    }

    /// Whether the shell reloads the page itself when its web content process dies: a live page
    /// does, like a browser tab; a still in progress has failed.
    pub fn reloads_on_crash(self) -> bool {
        self == Self::Page
    }
}

/// A response body: bytes held by the core, or a file range the shell reads itself, so large
/// media never crosses the FFI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Bytes(Vec<u8>),
    File {
        path: PathBuf,
        offset: u64,
        length: u64,
    },
}

impl Body {
    pub fn length(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File { length, .. } => *length,
        }
    }
}

/// The response to one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub status: u16,
    /// Content-Length included.
    pub headers: Vec<(&'static str, String)>,
    pub body: Body,
}

impl Reply {
    fn empty(status: u16) -> Self {
        Self {
            status,
            headers: vec![("Content-Length", "0".to_owned())],
            body: Body::Bytes(Vec::new()),
        }
    }
}

/// What a request URL refers to.
struct Address {
    host: String,
    /// The decoded path as log lines show it: "/scenes/a.html".
    shown: String,
    query: String,
}

impl Address {
    fn parse(url: &str) -> Self {
        let rest = url.split_once("://").map_or("", |(_, rest)| rest);
        let (rest, _fragment) = rest.split_once('#').unwrap_or((rest, ""));
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = rest
            .find('/')
            .map_or((rest, ""), |slash| rest.split_at(slash));
        let mut shown = decode_path(path);
        // Drop the trailing slash, as URL path resolution does.
        if shown.len() > 1 && shown.ends_with('/') {
            shown.pop();
        }
        Self {
            host: authority.to_owned(),
            shown,
            query: query.to_owned(),
        }
    }

    /// The path in the served folder.
    fn relative(&self) -> &str {
        self.shown.trim_start_matches('/')
    }
}

enum Found {
    /// A page's database query.
    Database,
    Text(String, &'static str),
    File(PathBuf, Rewrite),
    Refused,
    Missing,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rewrite {
    Not,
    Script,
    Page,
}

/// A path with `.`, `..` and empty parts resolved lexically, never above the root.
fn normalize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut out = PathBuf::new();
    for part in absolute.components() {
        match part {
            // Keep a Windows drive or share; without it the path refers to the current drive.
            Component::Prefix(_) | Component::RootDir => out.push(part),
            Component::Normal(name) => out.push(name),
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
        }
    }
    out
}

/// A served-folder path as pages and URLs name it, joined with `/` on every system.
fn name_in_folder(relative: &Path) -> String {
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The start of `path`: its drive or share (on Windows) and its root.
fn root_of(path: &Path) -> PathBuf {
    path.components()
        .take_while(|part| matches!(part, Component::Prefix(_) | Component::RootDir))
        .collect()
}

/// The most symlinks followed in one path, matching Linux. macOS follows fewer and Windows
/// more; a path with more counts as outside every folder.
const MOST_LINKS: u32 = 40;

/// Whether `file` (normalized) is `root` or inside it, also after following symlinks
/// (`real_root`; when that is `None`, nothing is inside).
fn within(root: &Path, real_root: Option<&Path>, file: &Path) -> bool {
    let real = (real_root, resolve(file));
    file.starts_with(root) && matches!(real, (Some(root), Some(real)) if real.starts_with(root))
}

/// `path` with every symlink followed as far as its components exist; missing components are
/// kept as they are. `None` after more than `MOST_LINKS` links, where the system's resolution
/// is unknown.
fn resolve(path: &Path) -> Option<PathBuf> {
    let mut out = root_of(path);
    let mut pending: Vec<OsString> = path.components().filter_map(normal_name).rev().collect();
    let mut hops = 0;
    while let Some(name) = pending.pop() {
        match name.to_str() {
            Some("..") => {
                out.pop();
                continue;
            }
            Some(".") => continue,
            _ => {}
        }
        let next = out.join(&name);
        let Ok(target) = std::fs::read_link(&next) else {
            out = next;
            continue;
        };
        if hops == MOST_LINKS {
            return None;
        }
        hops += 1;
        if target.has_root() {
            // A Windows target of `\x` is on the link's own drive.
            out = root_of(path).join(root_of(&target));
        }
        pending.extend(target.components().rev().filter_map(link_step));
    }
    Some(out)
}

/// A normal path component's name; `None` for a drive, root, `.` or `..`.
fn normal_name(part: Component) -> Option<OsString> {
    match part {
        Component::Normal(name) => Some(name.to_owned()),
        Component::Prefix(_) | Component::RootDir | Component::CurDir | Component::ParentDir => {
            None
        }
    }
}

/// A symlink target component as a step to follow: a name, or `..` to go up; `None` for a
/// drive, root or `.`.
fn link_step(part: Component) -> Option<OsString> {
    match part {
        Component::Normal(name) => Some(name.to_owned()),
        Component::ParentDir => Some(OsString::from("..")),
        Component::Prefix(_) | Component::RootDir | Component::CurDir => None,
    }
}

fn mime(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match extension.as_str() {
        "html" | "htm" => HTML,
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "css" => CSS,
        "txt" | "md" | "csv" | "srt" | "vtt" => "text/plain; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "tif" | "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/mp4",
        "ogg" => "audio/ogg",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "wasm" => "application/wasm",
        // On Windows and Linux, PDFs open in the system web view's built-in viewer.
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn rewrite_of(path: &Path) -> Rewrite {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_lowercase();
    match extension.as_str() {
        "html" | "htm" => Rewrite::Page,
        "js" | "mjs" | "css" => Rewrite::Script,
        _ => Rewrite::Not,
    }
}

/// The bytes of `count` that a `Range` header requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    Whole,
    Part { first: u64, last: u64 },
    Unsatisfiable,
}

/// Parses a single byte range (`bytes=0-99`, `bytes=100-`, `bytes=-50`). Multiple ranges, a
/// non-byte unit, or unparseable numbers get the whole file.
pub fn span(range: Option<&str>, count: u64) -> Span {
    let Some(asked) = range
        .and_then(|range| range.strip_prefix("bytes="))
        .filter(|asked| !asked.contains(','))
    else {
        return Span::Whole;
    };
    let count = i128::from(count);
    let (first, last) = asked_bytes(asked, count);
    if first <= last && first < count {
        Span::Part {
            first: first as u64,
            last: last as u64,
        }
    } else {
        Span::Unsatisfiable
    }
}

/// The first and last byte of `count` that `asked` (a range without `bytes=`) names, or the
/// whole file when its numbers do not parse.
fn asked_bytes(asked: &str, count: i128) -> (i128, i128) {
    let whole = (0, count - 1);
    let parts: Vec<&str> = asked.split('-').collect();
    let [start, end] = parts[..] else {
        return whole;
    };
    if let (true, Ok(suffix)) = (start.is_empty(), end.parse::<i64>()) {
        return ((count - i128::from(suffix)).max(0), count - 1);
    }
    let Ok(start) = start.parse::<i64>() else {
        return whole;
    };
    let last = end
        .parse::<i64>()
        .map_or(count - 1, |end| i128::from(end).min(count - 1));
    (i128::from(start), last)
}

/// Serves `nr://files/…` for one web view: its folder and nothing outside it, the pinned
/// libraries and fonts under `/__nr/`, and its entry document. A page has no other way to read
/// files, and no request leaves the machine. Shared across the shell's request threads.
pub struct Host {
    kind: Kind,
    /// The served folder, normalized, and with symlinks resolved (`None` when they cannot be, in
    /// which case nothing is served).
    root: PathBuf,
    real_root: Option<PathBuf>,
    /// The shell's resource folder, with `vendor/` and `fonts/`.
    resources: PathBuf,
    /// What `DOCUMENT_PATH` serves, for a host with its own document.
    document: Option<String>,
    /// The URL the web view loads first.
    entry: String,
    log: Mutex<Log>,
    /// A still host's current model; `None` for other hosts.
    model: Mutex<Option<StillModel>>,
}

/// A still host's current model, and the folder its file may read from.
struct StillModel {
    turn: u64,
    folder: PathBuf,
    real_folder: Option<PathBuf>,
}

impl Host {
    fn new(
        kind: Kind,
        root: &Path,
        resources: &Path,
        document: Option<String>,
        entry: String,
    ) -> Self {
        let root = normalize(root);
        let real_root = resolve(&root);
        Self {
            kind,
            root,
            real_root,
            resources: normalize(resources),
            document,
            entry,
            log: Mutex::new(Log::default()),
            model: Mutex::new(None),
        }
    }

    /// A host running the HTML file `file` as a live page, serving its folder.
    pub fn page(file: &Path, resources: &Path) -> Self {
        let file = normalize(file);
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let folder = file.parent().unwrap_or(&file).to_path_buf();
        Self::new(Kind::Page, &folder, resources, None, address_of(&name))
    }

    /// A host for the page that renders 3D model stills. It serves no folder of its own, only the
    /// last model passed to `draw_model` and the files beside it that the model reads.
    pub fn stills(resources: &Path) -> Self {
        Self::new(
            Kind::Still,
            resources,
            resources,
            Some(model_still_page()),
            format!("{ORIGIN}/{DOCUMENT_PATH}"),
        )
    }

    /// Passes a still host the model `file` to render next, in a new turn. Returns the JavaScript
    /// call that makes its page render it (`draw_model_call`) and the turn. The previous model
    /// stops being served.
    pub fn draw_model(&self, file: &Path) -> (String, u64) {
        let file = normalize(file);
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let folder = file.parent().unwrap_or(&file).to_path_buf();
        let mut model = self.model.relock();
        let turn = model.as_ref().map_or(1, |model| model.turn + 1);
        *model = Some(StillModel {
            turn,
            real_folder: resolve(&folder),
            folder,
        });
        let address = address_of(&format!("{MODEL_PATH}{turn}/{name}"));
        (draw_model_call(&address, turn), turn)
    }

    /// The URL the web view loads first.
    pub fn entry(&self) -> &str {
        &self.entry
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Log> {
        self.log.relock()
    }

    /// Logs `text` under `line`, once.
    pub fn add(&self, line: Line, text: String) {
        let mut log = self.log();
        let kept = &mut log.lines[line.index()];
        if kept.contains(&text) {
            return;
        }
        kept.push(text.clone());
        log.fresh.push((line, text));
    }

    /// Lines of one kind, in arrival order.
    pub fn lines(&self, line: Line) -> Vec<String> {
        self.log().lines[line.index()].clone()
    }

    /// The document it serves, which sets its web view's rules (`Kind::web`).
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The log as JSON: `{ blocked, refused, missing, errors, fonts: [line], requests,
    /// rangeRequests, served: [path] }`.
    pub fn log_json(&self) -> Value {
        let log = self.log();
        let lines = |line: Line| json!(log.lines[line.index()]);
        json!({
            "blocked": lines(Line::Blocked), "refused": lines(Line::Refused), "missing": lines(Line::Missing),
            "errors": lines(Line::Error), "fonts": lines(Line::Font),
            "requests": log.requests, "rangeRequests": log.range_requests, "served": log.served,
        })
    }

    /// Takes the oldest new log line; `None` when there is none.
    pub fn take_line(&self) -> Option<(Line, String)> {
        let mut log = self.log();
        (!log.fresh.is_empty()).then(|| log.fresh.remove(0))
    }

    /// Clears the log lines, keeping the counts and the served list.
    pub fn clear_log(&self) {
        let mut log = self.log();
        for line in Line::ALL {
            log.lines[line.index()].clear();
        }
    }

    /// Whether the web view may navigate to `url`; refusals are logged. `main_frame` is false for a
    /// subframe. Downloads are never allowed.
    pub fn navigate(&self, url: &str, main_frame: bool, download: bool) -> bool {
        if download {
            return false;
        }
        let scheme = scheme_of(url);
        let local = scheme == SCHEME || matches!(scheme.as_str(), "about" | "data" | "blob");
        let what = if main_frame { "navigation" } else { "frame" };
        let allowed = match self.kind {
            Kind::Page => local,
            Kind::Still => return scheme == SCHEME,
        };
        if !allowed {
            self.add(Line::Blocked, format!("{what} {url}"));
        }
        allowed
    }

    /// Handles a JSON message a page posted to the bridge, logging what it reports. Returns the
    /// replaced page when the message says one reloaded (`reloaded`).
    pub fn page_message(&self, message: &str) -> Option<String> {
        let Ok(Value::Object(body)) = serde_json::from_str::<Value>(message) else {
            return None;
        };
        let text = |key: &str| body.get(key).and_then(Value::as_str);
        match text("type")? {
            "blocked" => {
                let line = format!(
                    "{} {} ({})",
                    text("kind").unwrap_or("request"),
                    text("url").unwrap_or_default(),
                    text("page").unwrap_or_default()
                );
                self.add(Line::Blocked, line);
                None
            }
            "error" => {
                let said = text("text").unwrap_or_default();
                // A live page's errors name no page, since the page is the host's only file.
                let line =
                    text("page").map_or_else(|| said.to_owned(), |page| format!("{page}: {said}"));
                self.add(Line::Error, line);
                None
            }
            "reloaded" => Some(text("page").unwrap_or_default().to_owned()),
            _ => None,
        }
    }

    /// A file in the served folder, or `None` when the path escapes it (via `..`, an encoded `..`,
    /// or a symlink).
    pub fn confined(&self, relative: &str) -> Option<PathBuf> {
        let file = normalize(&self.root.join(relative));
        self.inside(&file).then_some(file)
    }

    /// Whether `file` (normalized) is the served folder or inside it, also after following
    /// symlinks.
    fn inside(&self, file: &Path) -> bool {
        within(&self.root, self.real_root.as_deref(), file)
    }

    /// A font file by name, from the folder's `fonts/` first, then the packaged fonts.
    fn font_file(&self, name: &str) -> Option<PathBuf> {
        let own = self.root.join("fonts");
        let file = normalize(&own.join(name));
        if file.starts_with(&own) && self.inside(&file) && file.is_file() {
            return Some(file);
        }
        let packaged = self.resources.join("fonts");
        let file = normalize(&packaged.join(name));
        (file.starts_with(&packaged) && file.is_file()).then_some(file)
    }

    /// The faces of `family` from the served folder's `fonts/`, else the packaged ones.
    pub fn faces(&self, family: &str) -> Vec<Face> {
        [self.root.join("fonts"), self.resources.join("fonts")]
            .iter()
            .map(|folder| faces(folder, family))
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    }

    /// The stylesheet a Google Fonts link represents, built from local files. `query` is the
    /// link's query string.
    pub fn stylesheet(&self, query: &str) -> String {
        let mut css = String::new();
        for family in asked_families(query)
            .iter()
            .filter(|family| !family.is_empty())
        {
            let found = self.faces(family);
            if found.is_empty() {
                self.add(Line::Font, format!("{family} is not packaged and not in the page's fonts/ folder; the page falls back to another face"));
                continue;
            }
            for face in found {
                css.push_str(&face.rule(family));
            }
        }
        css
    }

    fn find(&self, address: &Address) -> Found {
        if address.host != HOST {
            return Found::Refused;
        }
        let path = address.relative();
        if path == DOCUMENT_PATH {
            return self
                .document
                .clone()
                .map_or(Found::Missing, |document| Found::Text(document, HTML));
        }
        if path == DATABASE_PATH {
            return Found::Database;
        }
        if path == "__nr/fonts.css" {
            return Found::Text(self.stylesheet(&address.query), CSS);
        }
        if let Some(name) = path.strip_prefix("__nr/vendor/") {
            return self.vendor_file(name);
        }
        if let Some(name) = path.strip_prefix("__nr/fonts/") {
            return self
                .font_file(name)
                .map_or(Found::Missing, |file| Found::File(file, Rewrite::Not));
        }
        if let Some(rest) = path.strip_prefix(MODEL_PATH) {
            return self.model_file(rest);
        }
        // The rest of `__nr/` belongs to the host and holds nothing else; it never falls through
        // to the folder.
        if path.starts_with("__nr/") {
            return Found::Missing;
        }
        match self.kind {
            Kind::Page => self.folder_file(path),
            Kind::Still => Found::Refused,
        }
    }

    /// The file `<turn>/<path>` names beside a still host's current model, never outside its folder
    /// or from an earlier model.
    fn model_file(&self, rest: &str) -> Found {
        let model = self.model.relock();
        let Some(model) = model.as_ref() else {
            return Found::Refused;
        };
        let Some((turn, relative)) = rest.split_once('/') else {
            return Found::Refused;
        };
        if turn != model.turn.to_string() {
            return Found::Refused;
        }
        let file = normalize(&model.folder.join(relative));
        if !within(&model.folder, model.real_folder.as_deref(), &file) {
            return Found::Refused;
        }
        match std::fs::metadata(&file) {
            Ok(metadata) if metadata.is_file() => Found::File(file, Rewrite::Not),
            _ => Found::Missing,
        }
    }

    /// The pinned library `name` in the shell's `vendor/`, and nothing outside it.
    fn vendor_file(&self, name: &str) -> Found {
        let vendor = self.resources.join("vendor");
        let file = normalize(&vendor.join(name));
        if !file.starts_with(&vendor) || file == vendor {
            return Found::Refused;
        }
        if file.exists() {
            Found::File(file, Rewrite::Not)
        } else {
            Found::Missing
        }
    }

    /// The file at `path` in the served folder, and nothing outside it.
    fn folder_file(&self, path: &str) -> Found {
        let Some(file) = self.confined(path) else {
            return Found::Refused;
        };
        match std::fs::metadata(&file) {
            // A plain file only: a pipe, socket or device would hang or feed the reader.
            Ok(metadata) if metadata.is_file() => {
                let rewrite = rewrite_of(&file);
                Found::File(file, rewrite)
            }
            _ => Found::Missing,
        }
    }

    fn missing(&self, address: &Address) -> Reply {
        self.add(Line::Missing, address.relative().to_owned());
        Reply::empty(404)
    }

    /// Answers a request for `url` with its `Range` header, if any. It reads files and rewrites
    /// pages, so call it off the UI thread.
    pub fn answer(&self, url: &str, range: Option<&str>) -> Reply {
        let address = Address::parse(url);
        match self.find(&address) {
            Found::Refused => {
                self.add(
                    Line::Refused,
                    format!("{} is outside the page's folder", address.shown),
                );
                Reply::empty(403)
            }
            Found::Missing => self.missing(&address),
            Found::Database => {
                let answer = self
                    .ask(&address.query)
                    .unwrap_or_else(|why| json!({ "error": why }).to_string());
                self.respond(Content::Bytes(answer.into_bytes()), JSON, None)
            }
            Found::Text(text, mime) => self.respond(Content::Bytes(text.into_bytes()), mime, range),
            Found::File(file, rewrite) => match self.read(&file, rewrite) {
                Some(content) => {
                    if let Ok(relative) = file.strip_prefix(&self.root) {
                        self.log().served.insert(name_in_folder(relative));
                    }
                    self.respond(content, mime(&file), range)
                }
                None => self.missing(&address),
            },
        }
    }

    /// A page's query of a database in the served folder (`window.__nr.sql`, `page-data.js`). The
    /// URL's `q` parameter is `{"file": path in the folder, "run": [[sql, [values]], …]}`. Success
    /// returns `database::ask`'s JSON; failure returns the message the page rejects with. A live
    /// page can read and write, and its first write creates a missing database; a still host never
    /// queries. Neither the path nor the SQL can reach a file outside the folder.
    fn ask(&self, query: &str) -> Result<String, String> {
        let access = match self.kind {
            Kind::Page => Access::Creates,
            Kind::Still => return Err("a still asks no database".to_owned()),
        };
        let malformed = || "the call is not {file, run}".to_owned();
        let call = query
            .split('&')
            .find_map(|part| part.strip_prefix("q="))
            .and_then(decode_strict)
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .ok_or_else(malformed)?;
        let name = call
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(malformed)?;
        let statements = database::asked(call.get("run").ok_or_else(malformed)?)?;
        let file = self
            .confined(name.trim_start_matches('/'))
            .ok_or_else(|| format!("{name} is outside the page's folder"))?;
        if std::fs::metadata(&file).is_ok_and(|stat| !stat.is_file()) {
            return Err(format!("{name} is not a file"));
        }
        // Changes made by others since the page's last query are still reported after this
        // call; the call's own changes are not.
        let before = Version::of(&file);
        let answer = database::ask(&file, &statements, access);
        let mut log = self.log();
        if log.asked.get(&file).is_some_and(|known| *known != before) {
            log.asked_changed = true;
        }
        log.asked.insert(file.clone(), Version::of(&file));
        drop(log);
        if self.kind == Kind::Page {
            crate::page::watch::with(|pages| pages.asked(&file));
        }
        answer
    }

    /// Whether something other than the page changed a queried database since the page's last
    /// query. Each change is reported once.
    pub fn data_changed(&self) -> bool {
        let mut log = self.log();
        let mut changed = std::mem::take(&mut log.asked_changed);
        for (file, known) in &mut log.asked {
            let now = Version::of(file);
            if now != *known {
                *known = now;
                changed = true;
            }
        }
        changed
    }

    /// A served file's content: rewritten when it is a UTF-8 page, script or stylesheet, otherwise
    /// left on disk for the shell to read.
    fn read(&self, file: &Path, rewrite: Rewrite) -> Option<Content> {
        let mut handle = File::open(file).ok()?;
        let metadata = handle.metadata().ok()?;
        if !metadata.is_file() {
            return None;
        }
        if rewrite == Rewrite::Not {
            return Some(Content::File(file.to_path_buf(), metadata.len()));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or_default());
        handle.read_to_end(&mut bytes).ok()?;
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Some(Content::Bytes(bytes));
        };
        // Strip the byte order mark when reading as text.
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let out = match rewrite {
            Rewrite::Page => prepare_page(text),
            Rewrite::Script | Rewrite::Not => rewrite_links(text),
        };
        Some(Content::Bytes(out.into_bytes()))
    }

    /// Responds with the whole content or the requested byte range.
    fn respond(&self, content: Content, mime: &'static str, range: Option<&str>) -> Reply {
        let count = content.length();
        let span = span(range, count);
        {
            let mut log = self.log();
            log.requests += 1;
            if span != Span::Whole {
                log.range_requests += 1;
            }
        }
        let (first, last, status) = match span {
            Span::Whole => (0, count, 200),
            Span::Part { first, last } => (first, last + 1, 206),
            Span::Unsatisfiable => {
                return Reply {
                    status: 416,
                    headers: vec![
                        ("Content-Range", format!("bytes */{count}")),
                        ("Content-Length", "0".to_owned()),
                    ],
                    body: Body::Bytes(Vec::new()),
                };
            }
        };
        let body = match content {
            Content::Bytes(mut bytes) => {
                bytes.truncate(usize::try_from(last).unwrap_or(usize::MAX));
                Body::Bytes(bytes.split_off(usize::try_from(first).unwrap_or(usize::MAX)))
            }
            Content::File(path, _) => Body::File {
                path,
                offset: first,
                length: last - first,
            },
        };
        let mut headers = vec![
            ("Content-Type", mime.to_owned()),
            ("Accept-Ranges", "bytes".to_owned()),
            ("Access-Control-Allow-Origin", "*".to_owned()),
            ("Cache-Control", "no-store".to_owned()),
        ];
        if status == 206 {
            headers.push((
                "Content-Range",
                format!("bytes {first}-{}/{count}", last - 1),
            ));
        }
        headers.push(("Content-Length", body.length().to_string()));
        Reply {
            status,
            headers,
            body,
        }
    }
}

enum Content {
    Bytes(Vec<u8>),
    /// A file and its length.
    File(PathBuf, u64),
}

impl Content {
    fn length(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => bytes.len() as u64,
            Self::File(_, length) => *length,
        }
    }
}

/// A URL's scheme in lower case; empty when it has none.
fn scheme_of(url: &str) -> String {
    let Some((scheme, _)) = url.split_once(':') else {
        return String::new();
    };
    let mut characters = scheme.chars();
    let named = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character));
    if named {
        scheme.to_ascii_lowercase()
    } else {
        String::new()
    }
}

// CDN links that agents write by habit, rewritten to what the host serves.

/// Where agents link GSAP and three.js from.
const CDNS: [&str; 6] = [
    "cdnjs.cloudflare.com/ajax/libs",
    "cdn.jsdelivr.net/npm",
    "unpkg.com",
    "esm.sh",
    "cdn.skypack.dev",
    "ga.jspm.io/npm:",
];

/// Whether a character ends a version in a link: `/`, a quote, or whitespace (as ICU's `\s`).
fn ends_version(character: char) -> bool {
    matches!(character, '/' | '\'' | '"')
        || (character.is_whitespace() && character != '\u{0B}' && character != '\u{85}')
}

/// How a library's links encode its version.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Versioning {
    /// `@3.12.5` only.
    At,
    /// `@3.12.5` or `/3.12.5`.
    AtOrNumbered,
    /// `@3.12.5`, `/3.12.5` or a revision, `/r160`.
    AtNumberedOrRevision,
}

/// The possible remainders of `rest`: past a leading version in a form `versioning` allows, if
/// there is one, then `rest` unchanged.
fn versions(rest: &str, versioning: Versioning) -> Vec<&str> {
    let run = |text: &str| text.find(ends_version).unwrap_or(text.len());
    let mut out = Vec::new();
    if let Some(after) = rest.strip_prefix('@') {
        let length = run(after);
        if length > 0 {
            out.push(&after[length..]);
        }
    } else if let Some(after) = rest
        .strip_prefix('/')
        .filter(|_| versioning != Versioning::At)
    {
        let after_r = after
            .strip_prefix('r')
            .filter(|_| versioning == Versioning::AtNumberedOrRevision);
        for digits in after_r.into_iter().chain([after]) {
            if digits.starts_with(|character: char| character.is_ascii_digit()) {
                let tail = &digits[1..];
                out.push(&tail[run(tail)..]);
                break;
            }
        }
    }
    out.push(rest);
    out
}

/// `rest` after `name` and then `.min.js` or `.js`.
fn script_end<'a>(rest: &'a str, name: &str) -> Option<&'a str> {
    let rest = rest.strip_prefix(name)?;
    rest.strip_prefix(".min.js")
        .or_else(|| rest.strip_prefix(".js"))
}

/// A rewrite rule: for `text` starting with `http`, the length of the matched link and its
/// replacement.
type Rule = fn(&str) -> Option<(usize, String)>;

/// `text` after `https?://` and one of the CDNs.
fn after_cdn(text: &str) -> Option<&str> {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    CDNS.iter().find_map(|cdn| rest.strip_prefix(cdn))
}

/// `gsap/all.js` is the core here; plugins are their own files.
fn gsap_all(text: &str) -> Option<(usize, String)> {
    let rest = after_cdn(text)?.strip_prefix("/gsap")?;
    for rest in versions(rest, Versioning::AtOrNumbered) {
        let Some(rest) = rest.strip_prefix('/') else {
            continue;
        };
        for rest in [rest.strip_prefix("dist/"), Some(rest)]
            .into_iter()
            .flatten()
        {
            if let Some(end) = script_end(rest, "all") {
                return Some((text.len() - end.len(), format!("{}/gsap.min.js", gsap())));
            }
        }
    }
    None
}

fn gsap_file(text: &str) -> Option<(usize, String)> {
    let rest = after_cdn(text)?.strip_prefix("/gsap")?;
    for rest in versions(rest, Versioning::AtOrNumbered) {
        let Some(rest) = rest.strip_prefix('/') else {
            continue;
        };
        for rest in [rest.strip_prefix("dist/"), Some(rest)]
            .into_iter()
            .flatten()
        {
            let length = rest
                .find(|character: char| !character.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            if length == 0 {
                continue;
            }
            let name = &rest[..length];
            if let Some(end) = script_end(rest, name) {
                return Some((text.len() - end.len(), format!("{}/{name}.min.js", gsap())));
            }
        }
    }
    None
}

fn three_build(text: &str, build: &str, file: &str) -> Option<(usize, String)> {
    let rest = after_cdn(text)?.strip_prefix("/three")?;
    for rest in [rest.strip_prefix(".js"), Some(rest)].into_iter().flatten() {
        for rest in versions(rest, Versioning::AtNumberedOrRevision) {
            if let Some(end) = rest
                .strip_prefix("/build/")
                .and_then(|rest| script_end(rest, build))
            {
                return Some((text.len() - end.len(), format!("{}/{file}", three())));
            }
        }
    }
    None
}

fn three_module(text: &str) -> Option<(usize, String)> {
    three_build(text, "three.module", "three.module.js")
}

fn three_webgpu(text: &str) -> Option<(usize, String)> {
    three_build(text, "three.webgpu", "three.webgpu.js")
}

fn three_addons(text: &str) -> Option<(usize, String)> {
    let rest = after_cdn(text)?.strip_prefix("/three")?;
    versions(rest, Versioning::At)
        .into_iter()
        .find_map(|rest| rest.strip_prefix("/examples/jsm/"))
        .map(|end| (text.len() - end.len(), format!("{}/addons/", three())))
}

fn google_fonts(text: &str) -> Option<(usize, String)> {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?
        .strip_prefix("fonts.googleapis.com/css")?;
    let end = rest.strip_prefix("2?").or_else(|| rest.strip_prefix('?'))?;
    Some((text.len() - end.len(), format!("{ORIGIN}/__nr/fonts.css?")))
}

/// Replaces every link `rule` matches in `text`, left to right.
fn replace_links(text: &str, rule: Rule) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("http") {
        let at = search + found;
        match rule(&text[at..]) {
            Some((length, with)) => {
                out.push_str(&text[from..at]);
                out.push_str(&with);
                from = at + length;
                search = from;
            }
            None => search = at + 1,
        }
    }
    out.push_str(&text[from..]);
    out
}

/// Rewrites library and font links in any text a page loads (HTML, scripts, stylesheets): CDN
/// links to GSAP and three.js point to the pinned copies, and Google Fonts to local faces.
pub fn rewrite_links(text: &str) -> String {
    if !text.contains("http") {
        return text.to_owned();
    }
    let rules: [Rule; 6] = [
        gsap_all,
        gsap_file,
        three_module,
        three_webgpu,
        three_addons,
        google_fonts,
    ];
    rules
        .iter()
        .fold(text.to_owned(), |out, rule| replace_links(&out, *rule))
}

/// The first index of `needle` (lower-case ASCII) in `text` from `from`, ignoring ASCII case.
fn find_folded(text: &str, needle: &str, from: usize) -> Option<usize> {
    let (haystack, needle) = (text.as_bytes(), needle.as_bytes());
    (from..=haystack.len().checked_sub(needle.len())?)
        .find(|&at| haystack[at..at + needle.len()].eq_ignore_ascii_case(needle))
}

/// The end of the first `<tag…>` in `text` (`tag` in lower case), case-insensitively.
fn tag_end(text: &str, tag: &str) -> Option<usize> {
    let at = find_folded(text, tag, 0)?;
    text[at..].find('>').map(|close| at + close + 1)
}

/// Whether `text` has `type="importmap"`, allowing spaces around `=`, either quote, any case.
fn has_import_map(text: &str) -> bool {
    let space = |character: char| {
        character.is_whitespace() && character != '\u{0B}' && character != '\u{85}'
    };
    let mut from = 0;
    while let Some(at) = find_folded(text, "type", from) {
        from = at + 1;
        let rest = text[at + 4..].trim_start_matches(space);
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start_matches(space);
        let Some(rest) = rest.strip_prefix(['"', '\'']) else {
            continue;
        };
        if rest.len() >= 10
            && rest.as_bytes()[..9].eq_ignore_ascii_case(b"importmap")
            && matches!(rest.as_bytes()[9], b'"' | b'\'')
        {
            return true;
        }
    }
    false
}

/// `text` without links to Google's font hosts, since hints to them would be blocked and the
/// faces are local.
fn strip_font_hosts(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    let mut search = 0;
    while let Some(at) = find_folded(text, "<link", search) {
        let Some(close) = text[at..].find('>').map(|close| at + close) else {
            break;
        };
        // At least one character between `<link` and the host.
        let inside = &text[at + 5..close];
        let names_host = ["fonts.googleapis.com", "fonts.gstatic.com"]
            .iter()
            .any(|host| find_folded(inside, host, 1).is_some());
        if names_host {
            out.push_str(&text[from..at]);
            from = close + 1;
            search = from;
        } else {
            search = at + 1;
        }
    }
    out.push_str(&text[from..]);
    out
}

/// The import map added to a page without one, resolving bare `three` and `gsap` imports to the
/// pinned copies.
fn import_map() -> String {
    let (three, gsap) = (three(), gsap());
    format!(
        r#"<script type="importmap">{{"imports":{{"three":"{three}/three.module.js","three/webgpu":"{three}/three.webgpu.js","three/tsl":"{three}/three.tsl.js","three/addons/":"{three}/addons/","three/examples/jsm/":"{three}/addons/","gsap":"{gsap}/gsap.min.js"}}}}</script>"#
    )
}

/// A page as the host serves it: links rewritten, Google font hosts removed, and an import map
/// inserted after its `<head>` (or doctype, or at the start) unless it has one.
pub fn prepare_page(html: &str) -> String {
    let mut out = strip_font_hosts(&rewrite_links(html));
    if has_import_map(&out) {
        return out;
    }
    let at = tag_end(&out, "<head")
        .or_else(|| tag_end(&out, "<!doctype"))
        .unwrap_or(0);
    out.insert_str(at, &import_map());
    out
}

#[cfg(test)]
mod tests;
