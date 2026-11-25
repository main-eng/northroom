//! The drive: a folder on this computer, listed as it is.
//!
//! Every listed file and folder has an id built from its device and inode, so it survives a
//! rename by anyone and a restart, and a path that now holds a different file gets a new id.
//! Nothing about the drive's files is stored; ids live in memory only. Listing never walks a
//! subtree.

use crate::strings::{key, Key, Message};
use crate::threads::{each_on_threads, Spread};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The id of the drive's root folder, the same on every drive.
pub const DRIVE_ROOT_ID: &str = "b8e7f0e8-bb86-4bad-96c9-a9434ef96001";
const PAGE_EXTENSION: &str = ".md";
const TERMINAL_EXTENSION: &str = ".nterm";
/// Finder metadata, never listed even when hidden files are shown.
const NEVER_LISTED_NAME: &str = ".DS_Store";
/// Obsolete files that `Workspace::open` deletes from the metadata folder.
const RETIRED_FILES: [&str; 2] = ["catalog.json", "catalog.json.writing"];

/// Why a drive operation failed.
#[derive(Debug)]
pub enum DriveError {
    /// The drive refuses the change, with a localizable message for the person.
    Refused(Message),
    /// The system failed: the operation, the path and the system's message
    /// (`read /a/b: Permission denied (os error 13)`).
    Failed(String),
}

impl DriveError {
    /// The error in `language`; system messages stay verbatim.
    pub fn say(&self, language: &str) -> String {
        match self {
            Self::Refused(message) => message.say(language),
            Self::Failed(text) => text.clone(),
        }
    }
}

/// English, for agents and the log.
impl fmt::Display for DriveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(message) => message.fmt(formatter),
            Self::Failed(text) => formatter.write_str(text),
        }
    }
}

impl std::error::Error for DriveError {}

pub type DriveResult<T> = Result<T, DriveError>;

fn refused<T>(key: Key) -> DriveResult<T> {
    Err(DriveError::Refused(Message::new(key)))
}

/// A refusal whose message names `name`.
fn refusal_naming(key: Key, name: impl Into<String>) -> DriveError {
    DriveError::Refused(Message::new(key).with("name", name))
}

fn io_error(action: &str, path: &Path, cause: &io::Error) -> DriveError {
    DriveError::Failed(format!("{action} {}: {cause}", path.display()))
}

/// Whether an entry should be skipped: it vanished between listing and reading, or is an
/// unfollowable link. Its readable siblings are still listed.
fn is_skippable(cause: &io::Error) -> bool {
    matches!(
        cause.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
    ) || crate::drive::file_stat::is_link_loop(cause)
}

/// One listed file or folder, as known to this run.
#[derive(Clone, Debug)]
struct Record {
    id: String,
    path: PathBuf,
    device: String,
    inode: String,
}

/// A folder in the drive; `None` is the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub folder_id: Option<String>,
}

impl Location {
    pub const ROOT: Location = Location { folder_id: None };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Folder,
    /// A Markdown file.
    Page,
    /// A `.nterm` file (ADR-0005).
    Terminal,
    Image,
    Video,
    Audio,
    File,
}

impl EntryKind {
    /// Whether the entry is a folder whose files the drive lists. The list expands it in place,
    /// the columns show its files as the next column, and it can be the shown folder.
    pub fn holds_files(self) -> bool {
        self == Self::Folder
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: EntryKind,
    /// The display name: a page or terminal without its kind's extension, anything else as
    /// is.
    pub title: String,
    pub size_bytes: Option<u64>,
    pub modified_seconds: i64,
    /// Why the system protects this file from changes; `None` for an ordinary file.
    pub protection: Option<Protection>,
    /// A plain file's family by extension; `Other` otherwise.
    pub family: FileFamily,
    pub created_seconds: Option<i64>,
    pub accessed_seconds: Option<i64>,
    /// When the file's attributes last changed.
    pub changed_seconds: Option<i64>,
    /// The file's extension without the dot, as written; `None` for a folder or a bare name.
    pub extension: Option<String>,
    /// The folder the file is in, as a path.
    pub location: String,
    /// The owner's account name, or its number when it has no name.
    pub owner: String,
    /// The file's permission bits as four octal digits.
    pub permissions: String,
    /// File index data; `None` until requested.
    pub indexed: Option<Indexed>,
    /// The platform's kind name ("PNG image", "Markdown Document"), as in Finder's Kind column;
    /// `None` for a folder or when unavailable.
    pub kind_description: Option<String>,
}

/// Data only the system's file index has about a file. Every field may be unknown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Indexed {
    pub added_seconds: Option<i64>,
    pub last_opened_seconds: Option<i64>,
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub duration_seconds: Option<f64>,
    pub camera: Option<String>,
    pub authors: Vec<String>,
    pub comment: Option<String>,
    pub download_sources: Vec<String>,
}

impl Indexed {
    /// Parses the platform's output for one file: one `key<TAB>value` line per value, with a
    /// key repeated for multiple values. Unknown keys and malformed values are skipped.
    pub fn parse(text: &str) -> Self {
        let mut indexed = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('\t') else {
                continue;
            };
            if value.is_empty() {
                continue;
            }
            match key {
                "added" => {
                    indexed.added_seconds = value.parse::<f64>().ok().map(|seconds| seconds as i64)
                }
                "opened" => {
                    indexed.last_opened_seconds =
                        value.parse::<f64>().ok().map(|seconds| seconds as i64)
                }
                "width" => indexed.pixel_width = value.parse().ok(),
                "height" => indexed.pixel_height = value.parse().ok(),
                "duration" => {
                    indexed.duration_seconds = value
                        .parse()
                        .ok()
                        .filter(|seconds: &f64| seconds.is_finite())
                }
                "camera" => indexed.camera = Some(value.to_owned()),
                "author" => indexed.authors.push(value.to_owned()),
                "comment" => indexed.comment = Some(value.to_owned()),
                "source" => indexed.download_sources.push(value.to_owned()),
                _ => {}
            }
        }
        indexed
    }
}

/// Platform callback that queries the file index for one file.
pub type FileIndex = Box<dyn Fn(&Path) -> Option<Indexed> + Send + Sync>;

/// Platform callback returning a short description of the file at a path.
pub type FileWords = Box<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// Platform callback naming a file's kind, as Finder's Kind column does.
pub type KindDescriber = FileWords;

/// What the system prevents a person from changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    /// A system item: browse only.
    System,
    /// A standard location: it cannot move, but its contents can be organised.
    Location,
    /// The protection could not be read, so nothing changes until it can.
    Unavailable,
}

impl Protection {
    /// The message shown for it.
    pub fn words(self) -> Key {
        match self {
            Self::System => key::DRIVE_PROTECTED_SYSTEM,
            Self::Location => key::DRIVE_PROTECTED_LOCATION,
            Self::Unavailable => key::DRIVE_PROTECTION_UNKNOWN,
        }
    }

    /// Whether the item's contents are protected as well as the item.
    pub fn protects_contents(self) -> bool {
        matches!(self, Self::System | Self::Unavailable)
    }
}

const MAC_SYSTEM_DIRECTORIES: [&str; 8] = [
    "System", "bin", "sbin", "dev", "etc", "private", "var", "usr",
];
const MAC_STANDARD_LOCATIONS: [&str; 8] = [
    "Applications",
    "Library",
    "Users",
    "Volumes",
    "opt",
    "home",
    "cores",
    "tmp",
];
const MAC_DATA_VOLUME_PREFIX: &str = "System/Volumes/Data/";
const MAC_LOCAL_DIRECTORY: &str = "usr/local";

fn contains_path(directory: &str, path: &str) -> bool {
    path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Protection by location: the drive root, the home folder and its ancestors, and the system
/// folders when the drive is the whole disk.
pub fn path_protection(path: &Path, root: &Path, home: &Path, mac: bool) -> Option<Protection> {
    if path == root {
        return Some(Protection::Location);
    }
    let location = match (mac, path.strip_prefix(root)) {
        (true, Ok(relative)) => {
            let relative = relative.to_string_lossy();
            // The writable data volume exposes the same places under a second path.
            let on_drive = relative
                .strip_prefix(MAC_DATA_VOLUME_PREFIX)
                .unwrap_or(&relative);
            if let Some(protection) = mac_disk_protection(root, on_drive) {
                return Some(protection);
            }
            root.join(on_drive)
        }
        _ => path.to_path_buf(),
    };
    (location == home || (home.starts_with(&location) && home != location))
        .then_some(Protection::Location)
}

/// Mac protection for `on_drive`, a path relative to `root`, when it is a system folder or a
/// standard place. Only a whole-disk drive has the system folders at its top.
fn mac_disk_protection(root: &Path, on_drive: &str) -> Option<Protection> {
    if root != Path::new("/") {
        return None;
    }
    if !contains_path(MAC_LOCAL_DIRECTORY, on_drive)
        && MAC_SYSTEM_DIRECTORIES
            .iter()
            .any(|directory| contains_path(directory, on_drive))
    {
        return Some(Protection::System);
    }
    (MAC_STANDARD_LOCATIONS.contains(&on_drive) || on_drive == MAC_LOCAL_DIRECTORY)
        .then_some(Protection::Location)
}

#[derive(Clone, Debug)]
pub struct Crumb {
    pub location: Location,
    pub title: String,
}

/// A file's place in the drive (`Workspace::place_of`).
#[derive(Clone, Debug)]
pub struct Placed {
    pub title: String,
    pub folder: Location,
    /// The folders from the root down to `folder`.
    pub folders: Vec<Crumb>,
}

/// Whether `directory` lists `name`. Dot-names are hidden, as in Finder, unless hidden files are
/// shown, and so are SQLite's companion files (`database::is_companion`), which belong to the
/// database and come and go with every write.
fn is_name_listed(directory: &Path, name: &str, hidden_shown: bool) -> bool {
    name != NEVER_LISTED_NAME
        && (hidden_shown
            || !(name.starts_with('.') || crate::sheets::database::is_companion(directory, name)))
}

fn ends_with_ignoring_case(name: &str, extension: &str) -> bool {
    name.len() >= extension.len()
        && name.is_char_boundary(name.len() - extension.len())
        && name[name.len() - extension.len()..].eq_ignore_ascii_case(extension)
}

/// The name without a Northroom kind extension.
fn format_title(name: &str) -> &str {
    [PAGE_EXTENSION, TERMINAL_EXTENSION]
        .iter()
        .find(|extension| ends_with_ignoring_case(name, extension) && name.len() > extension.len())
        .map_or(name, |extension| &name[..name.len() - extension.len()])
}

/// The media type of a drive file, by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Image,
    Video,
    Audio,
    /// A 3D model (`.glb`).
    Model,
}

/// Still image extensions this platform decodes, so the app shows them as pictures: Image I/O on
/// a Mac (`kit::platform::mac::image_thumbnail`, `media::apple::picture`) and WIC on Windows
/// (`media::windows::picture`), both including HEIC and AVIF; on Linux the `image` crate
/// (`kit::platform::portable::image_thumbnail`, `media::picture`), matching its `Cargo.toml`
/// features. A still a platform cannot decode is a plain file there.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const DECODED_STILL_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "webp", "gif", "heic", "heif", "avif", "tif", "tiff", "bmp",
];
#[cfg(target_os = "linux")]
const DECODED_STILL_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "webp", "gif", "tif", "tiff", "bmp"];

/// Whether the platform decodes a still with this extension (lowercase, no dot). On Windows,
/// WebP, HEIC and AVIF need optional system codecs, so they count only where WIC can decode them
/// (`media::wic_decodes_extension`); otherwise they are plain files, not blank pictures.
pub(crate) fn decodes_still(extension: &str) -> bool {
    let listed = DECODED_STILL_EXTENSIONS.contains(&extension);
    #[cfg(target_os = "windows")]
    let listed = listed && crate::media::wic_decodes_extension(extension);
    listed
}

/// The media type for this extension (lowercase, no dot); `None` otherwise. A picture is any
/// decodable still, plus SVG, which is drawn separately. `.ts` is far more often TypeScript than
/// a transport stream, so it is never media. `.mka` is Matroska audio; `.mkv` and `.webm` are
/// video by name, which `media_of_file` refines.
pub fn media_of_extension(extension: &str) -> Option<Media> {
    match extension {
        "svg" => Some(Media::Image),
        still if decodes_still(still) => Some(Media::Image),
        "mp4" | "m4v" | "mov" | "qt" | "mkv" | "webm" | "ogv" | "m2ts" => Some(Media::Video),
        "mp3" | "wav" | "m4a" | "ogg" | "opus" | "aac" | "adts" | "flac" | "mka" => {
            Some(Media::Audio)
        }
        "glb" => Some(Media::Model),
        _ => None,
    }
}

/// Whether to read a picture's size from its header when the file index has none: every
/// decodable still, which excludes SVG (it has no pixel size).
pub fn reads_picture_size(extension: &str) -> bool {
    decodes_still(extension)
}

/// Whether phone stills (`heic`, `heif`, `avif`) are pictures on the test machine: always on a
/// Mac, never on Linux, and on Windows when WIC has the codecs (a CI runner may not).
#[cfg(test)]
pub(crate) fn phone_still_decodes_here(extension: &str) -> bool {
    #[cfg(target_os = "windows")]
    return crate::media::wic_decodes_extension(extension);
    #[cfg(not(target_os = "windows"))]
    {
        let _ = extension;
        cfg!(target_os = "macos")
    }
}

/// Extensions used for both Matroska video and audio-only files (a WebM voice note).
const MATROSKA_FILM_EXTENSIONS: [&str; 2] = ["mkv", "webm"];

/// The media type of `path` by its extension, case-insensitively. For `.mkv` and `.webm` the
/// headers decide: no video track means audio. Headers are read once per file version; if they
/// cannot be read, the name decides.
pub fn media_of_file(path: &Path) -> Option<Media> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let media = media_of_extension(&extension)?;
    if MATROSKA_FILM_EXTENSIONS.contains(&extension.as_str())
        && crate::media::matroska_holds_video(path) == Some(false)
    {
        return Some(Media::Audio);
    }
    Some(media)
}

/// How a listed path appears in a listing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    Folder,
    /// A file, or on a Mac a folder Finder treats as one (an `.app` or a document package
    /// such as `.rtfd` or `.pages`, `is_package`).
    File,
}

impl Shape {
    /// The shape of `path` with lowercase `extension`, from `stat`.
    fn of(stat: &fs::Metadata, path: &Path, extension: &str) -> Self {
        if !stat.is_dir() || is_package(path, extension) {
            return Self::File;
        }
        Self::Folder
    }
}

/// The marker entry inside an application bundle.
const APPLICATION_MARKER: &str = "Contents";

/// Entries that mark a document package, at least one required: a rich text package's text, or
/// an iWork document's index (a zip, a folder or gzipped XML depending on the iWork version).
const DOCUMENT_PACKAGE_MARKERS: [&str; 4] = ["TXT.rtf", "Index.zip", "Index", "index.xml.gz"];

/// Whether the folder at `path` with lowercase `extension` is a single file to Finder: an
/// application or document package by both name and contents, so a plain folder named
/// `deploy.key` stays a folder. Mac only; elsewhere every folder is a folder.
fn is_package(path: &Path, extension: &str) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    if extension == "app" {
        return path.join(APPLICATION_MARKER).is_dir();
    }
    DOCUMENT_PACKAGE_EXTENSIONS.contains(&extension)
        && DOCUMENT_PACKAGE_MARKERS
            .iter()
            .any(|marker| path.join(marker).exists())
}

/// The kind of the file at `path` with lowercase `extension`: page, terminal, or by media.
fn file_kind(path: &Path, extension: &str) -> EntryKind {
    if extension == &PAGE_EXTENSION[1..] {
        return EntryKind::Page;
    }
    if extension == &TERMINAL_EXTENSION[1..] {
        return EntryKind::Terminal;
    }
    media_kind(path)
}

fn media_kind(path: &Path) -> EntryKind {
    match media_of_file(path) {
        Some(Media::Image) => EntryKind::Image,
        Some(Media::Video) => EntryKind::Video,
        Some(Media::Audio) => EntryKind::Audio,
        Some(Media::Model) | None => EntryKind::File,
    }
}

/// A plain file's family by extension, which sets its row and card glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileFamily {
    Other,
    /// Source code, markup, styles or structured configuration (`is_code_extension`).
    Code,
    /// An archive, as Compress writes.
    Archive,
}

impl FileFamily {
    /// The family for this extension (lowercase, no dot).
    pub fn of_extension(extension: &str) -> Self {
        if is_code_extension(extension) {
            Self::Code
        } else if matches!(
            extension,
            "zip" | "tar" | "gz" | "tgz" | "bz2" | "tbz2" | "xz" | "txz" | "zst" | "7z" | "rar"
        ) {
            Self::Archive
        } else {
            Self::Other
        }
    }

    /// The family of the file at `path`.
    pub fn of_path(path: &Path) -> Self {
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        Self::of_extension(&extension)
    }
}

/// Whether this extension (lowercase, no dot) is source code, markup, styles or structured
/// configuration, shown with the code glyph.
pub fn is_code_extension(extension: &str) -> bool {
    matches!(
        extension,
        "ts" | "tsx"
            | "mts"
            | "cts"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "rb"
            | "go"
            | "rs"
            | "java"
            | "kt"
            | "swift"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "m"
            | "mm"
            | "cs"
            | "php"
            | "lua"
            | "dart"
            | "zig"
            | "scala"
            | "ex"
            | "exs"
            | "hs"
            | "clj"
            | "r"
            | "pl"
            | "sql"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "html"
            | "htm"
            | "xml"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "vue"
            | "svelte"
            | "wgsl"
            | "glsl"
            | "frag"
            | "vert"
            | "json"
            | "jsonc"
            | "json5"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "graphql"
            | "gql"
            | "proto"
            | "tf"
    )
}

/// Whether `path` is a PDF by name. A PDF opens as pages drawn by the platform.
pub fn is_pdf_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

/// Whether `path` is a file the platform previewer draws and Northroom does not, by name
/// (`SYSTEM_PREVIEW_EXTENSIONS`): Quick Look on a Mac, as Finder's space bar shows. It is
/// view-only.
pub fn is_system_preview_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|extension| SYSTEM_PREVIEW_EXTENSIONS.contains(&extension.as_str()))
}

/// Formats Quick Look previews that Northroom has no view for. Formats the app draws itself
/// (pages, text, tables, decks, PDFs, decodable pictures, media, `.glb`) are excluded.
// One kind of file per line, under its comment.
#[rustfmt::skip]
const SYSTEM_PREVIEW_EXTENSIONS: &[&str] = &[
    // Presentations the deck cannot read: legacy PowerPoint, Keynote and OpenDocument.
    "ppt", "pps", "pot", "key",
    // Documents the page view cannot read: legacy Word, rich text, Pages and OpenDocument.
    "doc", "pages",
    // Spreadsheets the table cannot read: legacy Excel and Numbers.
    "xls", "numbers",
    // Fonts.
    "ttf", "otf", "ttc", "otc", "dfont",
    // Pictures the app cannot decode: Photoshop files and camera raw photos.
    "psd", "dng", "cr2", "cr3", "nef", "arw", "raf", "orf", "rw2",
    // 3D scenes Model I/O reads.
    "usdz", "usd", "usda", "usdc", "obj", "stl", "ply", "abc",
    // Contacts and calendar events.
    "vcf", "ics",
];

/// Document packages: folders on disk that Finder and the app list as one file.
pub(crate) const DOCUMENT_PACKAGE_EXTENSIONS: [&str; 4] = ["rtfd", "pages", "numbers", "key"];

/// The largest file read as text.
pub const TEXT_PREVIEW_MAX_BYTES: u64 = 1024 * 1024;

/// How a plain file is read, for its card and its view. Kinds are tried in the order below, so
/// a PDF, picture or model is never read as text however plain its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileRead {
    /// A picture, video, audio or 3D model, by name.
    Media(Media),
    /// A PDF, by name: pages when opened, glyph and name on its card.
    Pdf,
    /// A file the platform previewer draws, by name (`is_system_preview_file`), where one
    /// exists. Elsewhere such a file opens in its own application.
    SystemPreview,
    /// An Excel or OpenDocument workbook up to `workbook::FILE_MAX_BYTES`, by name: a table
    /// when opened.
    Workbook,
    /// A file drawn as pages (`deck::Kind`) up to `deck::FILE_MAX_BYTES`, by name: its pages
    /// when opened.
    Deck,
    /// A SQLite database, by contents (`database::is_database`): tables when opened. The
    /// string is the top of its first table for the card (empty when there is no table or it is
    /// unreadable).
    Database(String),
    /// Text: a page-style card and its own view when opened.
    Text(String),
    /// An HTML page: run live when selected or opened, glyph and name on its card.
    Html(String),
    /// Anything else: glyph and name, opened in its own application.
    External,
}

impl FileRead {
    /// Whether the file uses the system thumbnail Finder or Explorer shows: a file Northroom
    /// draws nothing of, a PDF, a workbook, or anything external. Where the system has only an
    /// icon, the glyph stays.
    pub fn shows_system_thumbnail(&self) -> bool {
        match self {
            FileRead::Pdf
            | FileRead::SystemPreview
            | FileRead::Workbook
            | FileRead::Deck
            | FileRead::External => true,
            FileRead::Media(_) | FileRead::Text(_) | FileRead::Html(_) | FileRead::Database(_) => {
                false
            }
        }
    }
}

/// Whether `path` uses the system thumbnail (`FileRead::shows_system_thumbnail`), which a shell
/// asks before fetching one for a row. It reads the file, so call it off the main thread.
pub fn shows_system_thumbnail(path: &Path) -> bool {
    read_file(path).shows_system_thumbnail()
}

/// Reads the file at `path`; see `FileRead`.
pub fn read_file(path: &Path) -> FileRead {
    let Ok(stat) = fs::metadata(path) else {
        return FileRead::External;
    };
    if !stat.is_file() {
        return read_folder_as_file(path, &stat);
    }
    if let Some(read) = read_by_name(path, stat.len()) {
        return read;
    }
    if crate::sheets::database::is_database(path) {
        return FileRead::Database(crate::sheets::database::card_text(path));
    }
    read_text_preview(path, stat.len())
}

/// A folder listed as a file (`Shape::File`): a document package the app draws shows as pages,
/// another uses the previewer; an application or anything else opens on its own.
fn read_folder_as_file(path: &Path, stat: &fs::Metadata) -> FileRead {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let package = stat.is_dir()
        && DOCUMENT_PACKAGE_EXTENSIONS.contains(&extension.as_str())
        && is_package(path, &extension);
    let drawn = path
        .file_name()
        .is_some_and(|name| crate::deck::Kind::of_name(&name.to_string_lossy()).is_some());
    match (package, drawn) {
        (true, true) => FileRead::Deck,
        (true, false) => FileRead::SystemPreview,
        (false, _) => FileRead::External,
    }
}

/// How `path`, `length_bytes` long, is read by name alone: media, PDF, system preview or
/// workbook. `None` when the name decides nothing.
fn read_by_name(path: &Path, length_bytes: u64) -> Option<FileRead> {
    if let Some(media) = media_of_file(path) {
        return Some(FileRead::Media(media));
    }
    if is_pdf_file(path) {
        return Some(FileRead::Pdf);
    }
    // Off a Mac there is no previewer, so the file opens in its own application; its bytes
    // are never shown as text.
    if is_system_preview_file(path) {
        return Some(if cfg!(target_os = "macos") {
            FileRead::SystemPreview
        } else {
            FileRead::External
        });
    }
    read_document(path, length_bytes)
}

/// How a presentation or workbook, by name, is read: as its own kind when small enough to
/// show, else as a file with only its details. `None` for any other name.
fn read_document(path: &Path, length_bytes: u64) -> Option<FileRead> {
    let name = path.file_name()?.to_string_lossy();
    let (shown, max_bytes) = if crate::deck::Kind::of_name(&name).is_some() {
        (FileRead::Deck, crate::deck::FILE_MAX_BYTES)
    } else if crate::sheets::workbook::is_workbook_name(&name) {
        (FileRead::Workbook, crate::sheets::workbook::FILE_MAX_BYTES)
    } else {
        return None;
    };
    Some(if length_bytes <= max_bytes {
        shown
    } else {
        FileRead::External
    })
}

/// Reads `path`, `length_bytes` long, as text or a live page when small enough, NUL-free and
/// valid UTF-8; otherwise external.
fn read_text_preview(path: &Path, length_bytes: u64) -> FileRead {
    if length_bytes > TEXT_PREVIEW_MAX_BYTES {
        return FileRead::External;
    }
    let Ok(bytes) = fs::read(path) else {
        return FileRead::External;
    };
    if bytes.len() as u64 > TEXT_PREVIEW_MAX_BYTES || bytes.contains(&0) {
        return FileRead::External;
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return FileRead::External;
    };
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(extension.as_deref(), Some("html" | "htm")) {
        FileRead::Html(text)
    } else {
        FileRead::Text(text)
    }
}

/// The text of `path` when it opens as text (`FileRead::Text` or `Html`): at most
/// `TEXT_PREVIEW_MAX_BYTES`, NUL-free, valid UTF-8, and not a PDF, media or workbook by name.
pub fn read_text_file(path: &Path) -> Option<String> {
    match read_file(path) {
        FileRead::Text(text) | FileRead::Html(text) => Some(text),
        FileRead::Media(_)
        | FileRead::Pdf
        | FileRead::SystemPreview
        | FileRead::Workbook
        | FileRead::Deck
        | FileRead::Database(_)
        | FileRead::External => None,
    }
}

/// How a non-page text file is shown, chosen by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextForm {
    /// Verbatim monospace text: source code and every name not listed below.
    Source,
    /// `.txt` and `.text`: a read-only page titled by the file name without extension, with a
    /// paragraph per block of lines between blank lines.
    PlainPage,
    /// `.csv` and `.tsv`: a table.
    Table,
}

/// The form for a text file named `name`. A name with no dot is its own extension, so `README`
/// is source.
pub fn text_form(name: &str) -> TextForm {
    let extension = name.rsplit('.').next().unwrap_or(name).to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "text" => TextForm::PlainPage,
        delimited if crate::sheets::spreadsheet::is_delimited(delimited) => TextForm::Table,
        _ => TextForm::Source,
    }
}

/// A file name without extension, as a plain-text page is titled. A name whose only dot is
/// leading keeps it.
pub fn name_without_extension(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    }
}

/// Name sort order: by UTF-16 code unit.
pub(crate) fn compare_names(left: &str, right: &str) -> std::cmp::Ordering {
    left.encode_utf16().cmp(right.encode_utf16())
}

/// Platform callback that moves a file or folder to the system trash; the drive never deletes
/// outright. Returns the item's new location in the trash when known.
pub type Trash = Box<dyn Fn(&Path) -> io::Result<Option<PathBuf>> + Send + Sync>;

/// A test trash: files move into `folder` under their name, numbered if taken, and never reach
/// the system trash.
pub fn trash_into(folder: PathBuf) -> Trash {
    Box::new(move |path| {
        fs::create_dir_all(&folder)?;
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::other("the path has no name"))?;
        let mut target = folder.join(name);
        let mut number = 2;
        while target.exists() {
            target = folder.join(format!("{} {number}", name.to_string_lossy()));
            number += 1;
        }
        fs::rename(path, &target)?;
        Ok(Some(target))
    })
}

/// The ids of a trashed file and its contents, so restoring it keeps them.
#[derive(Clone, Debug)]
pub struct Kept {
    /// The file's original path in the drive.
    path: PathBuf,
    records: Vec<Record>,
    /// A database's companion files trashed with it (`database::companions`): each suffix and
    /// where the trash put it.
    companions: Vec<(&'static str, PathBuf)>,
}

/// A trashed file: where the trash put it, when known, and what the drive kept of it.
#[derive(Clone, Debug)]
pub struct Trashed {
    pub landed: Option<PathBuf>,
    pub kept: Kept,
}

/// What a copy brings: a file's bytes, a folder recursively, or a link as a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CopyKind {
    File,
    Folder,
    Link,
}

/// Creates a link at `link` to `target`, like the link at `source`.
#[cfg(unix)]
fn make_link(target: &Path, _source: &Path, link: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Creates a link at `link` to `target`, like the link at `source`, as a directory or file link
/// since Windows distinguishes them.
#[cfg(windows)]
fn make_link(target: &Path, source: &Path, link: &Path) -> io::Result<()> {
    if fs::metadata(source).is_ok_and(|stat| stat.is_dir()) {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

/// Copies the contents of `source` into the new folder `destination`: folders recursively,
/// files with bytes and permissions, links as links. The folder's permissions are applied last,
/// so a read-only folder can still be filled.
fn copy_tree(source: &Path, destination: &Path) -> DriveResult<()> {
    let read = |cause: io::Error| io_error("read", source, &cause);
    for entry in fs::read_dir(source).map_err(read)? {
        let entry = entry.map_err(read)?;
        let (from, to) = (entry.path(), destination.join(entry.file_name()));
        let copy = |cause: io::Error| io_error("copy", &from, &cause);
        let kind = entry.file_type().map_err(copy)?;
        if kind.is_symlink() {
            let target = fs::read_link(&from).map_err(copy)?;
            make_link(&target, &from, &to).map_err(copy)?;
        } else if kind.is_dir() {
            fs::create_dir(&to).map_err(copy)?;
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to).map_err(copy)?;
        } else {
            return Err(refusal_naming(
                key::DRIVE_NOT_A_FILE,
                entry.file_name().to_string_lossy(),
            ));
        }
    }
    let permissions = fs::metadata(source).map_err(read)?.permissions();
    fs::set_permissions(destination, permissions)
        .map_err(|cause| io_error("copy", destination, &cause))
}

/// How many "Name N" variants a new file tries before giving up.
const NUMBERED_NAME_ATTEMPTS: u32 = 100;

fn require_name(name: &str) -> DriveResult<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return refused(key::DRIVE_FILENAME_HAS_PATH);
    }
    Ok(())
}

/// Listing parallelism: up to four threads with at least 64 files each, so a small folder is
/// read on the calling thread.
pub(crate) const LISTING_SPREAD: Spread = Spread {
    most_threads: 4,
    fewest_items_per_thread: 64,
};

/// The Northroom kind extension `name` ends in, if any.
fn named_file_extension(name: &str) -> Option<&'static str> {
    [PAGE_EXTENSION, TERMINAL_EXTENSION]
        .into_iter()
        .find(|extension| ends_with_ignoring_case(name, extension) && name.len() > extension.len())
}

/// A file id built from its system identity (`file_stat::identity`).
fn identity_id(device: &str, inode: &str) -> String {
    format!("{device}-{inode}")
}

pub struct Workspace {
    root: PathBuf,
    root_label: String,
    /// The app's per-drive data folder (view state and the like).
    metadata_directory: PathBuf,
    records: HashMap<String, Record>,
    ids_by_path: HashMap<PathBuf, String>,
    hidden_shown: bool,
    trash: Option<Trash>,
    /// The person's home folder, which cannot move, nor can its ancestors.
    home: PathBuf,
    /// The platform file index, and whether listings query it. It is queried once per file, so
    /// only while something shown needs it.
    index: Option<FileIndex>,
    index_wanted: bool,
    kind_describer: Option<KindDescriber>,
}

impl Workspace {
    /// Opens the drive at `root` with its data in `metadata_directory`, deleting obsolete files
    /// there (`RETIRED_FILES`).
    pub fn open(root: &Path, metadata_directory: &Path) -> DriveResult<Self> {
        for name in RETIRED_FILES {
            let retired = metadata_directory.join(name);
            match fs::remove_file(&retired) {
                Err(cause) if cause.kind() != io::ErrorKind::NotFound => {
                    eprintln!("{} was not deleted: {cause}", retired.display());
                }
                _ => {}
            }
        }
        let root =
            fs::canonicalize(root).map_err(|cause| io_error("open the drive at", root, &cause))?;
        let root_label = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        Ok(Self {
            root,
            root_label,
            metadata_directory: metadata_directory.to_path_buf(),
            records: HashMap::new(),
            ids_by_path: HashMap::new(),
            hidden_shown: false,
            trash: None,
            home: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_default(),
            index: None,
            index_wanted: false,
            kind_describer: None,
        })
    }

    /// The resolved root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The app's per-drive data folder.
    pub fn metadata_directory(&self) -> &Path {
        &self.metadata_directory
    }

    pub fn hidden_shown(&self) -> bool {
        self.hidden_shown
    }

    pub fn root_label(&self) -> &str {
        &self.root_label
    }

    pub fn set_hidden_shown(&mut self, shown: bool) {
        self.hidden_shown = shown;
    }

    pub fn set_file_index(&mut self, index: FileIndex) {
        self.index = Some(index);
    }

    pub fn set_kind_describer(&mut self, describer: KindDescriber) {
        self.kind_describer = Some(describer);
    }

    /// The platform's kind name for `path`, if available.
    pub fn kind_description_at(&self, path: &Path) -> Option<String> {
        self.kind_describer
            .as_ref()
            .and_then(|describe| describe(path))
    }

    /// Sets whether listings query the file index for each file.
    pub fn set_index_wanted(&mut self, wanted: bool) {
        self.index_wanted = wanted;
    }

    pub fn index_wanted(&self) -> bool {
        self.index_wanted
    }

    /// Queries the file index for `id` now.
    pub fn indexed(&mut self, id: &str) -> Option<Indexed> {
        let path = self.path_of(id).ok()?;
        self.index_of(&path, media_kind(&path))
    }

    /// File index data for `path`, of `kind`. A video's size and duration missing from the index
    /// (in an unindexed folder) are read from the file, once per file version.
    fn index_of(&self, path: &Path, kind: EntryKind) -> Option<Indexed> {
        let indexed = self.index.as_ref()?(path);
        if kind != EntryKind::Video {
            return indexed;
        }
        let mut indexed = indexed.unwrap_or_default();
        let sized = indexed.pixel_width.is_some() && indexed.pixel_height.is_some();
        if !sized || indexed.duration_seconds.is_none() {
            let probed = crate::media::probe_once(path);
            if let Some((width, height)) = probed.dimensions.filter(|_| !sized) {
                (indexed.pixel_width, indexed.pixel_height) = (Some(width), Some(height));
            }
            indexed.duration_seconds = indexed.duration_seconds.or(probed.duration_seconds);
        }
        (indexed != Indexed::default()).then_some(indexed)
    }

    pub fn set_trash(&mut self, trash: Trash) {
        self.trash = Some(trash);
    }

    /// The record for `id`, which must be directly in `directory`.
    fn require_child(&mut self, id: &str, directory: &Path) -> DriveResult<Record> {
        let record = self.require_record(id)?;
        if record.path.parent() != Some(directory) {
            return refused(key::DRIVE_FILE_MOVED_OUT);
        }
        Ok(record)
    }

    fn require_vacant(path: &Path) -> DriveResult<()> {
        match fs::symlink_metadata(path) {
            Err(cause) if cause.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(cause) => Err(io_error("read", path, &cause)),
            Ok(_) => Err(refusal_naming(
                key::DRIVE_NAME_TAKEN,
                path.file_name().unwrap_or_default().to_string_lossy(),
            )),
        }
    }

    /// Moves a listed file to `destination`, keeping its id and its contents' ids.
    fn move_record(&mut self, record: &Record, destination: &Path) -> DriveResult<()> {
        if record.path == destination {
            return Ok(());
        }
        self.assert_contained(destination)?;
        if destination.starts_with(&record.path) {
            return refused(key::DRIVE_FOLDER_INTO_ITSELF);
        }
        Self::require_vacant(destination)?;
        self.require_record(&record.id)?;
        self.assert_mutable(record)?;
        let source = record.path.clone();
        // A database moves with its SQLite companion files.
        let companions = self.companions_to_take(&source, destination)?;
        fs::rename(&source, destination).map_err(|cause| io_error("move", &source, &cause))?;
        for (suffix, companion) in &companions {
            fs::rename(
                companion,
                crate::sheets::database::beside(destination, suffix),
            )
            .map_err(|cause| io_error("move", companion, &cause))?;
        }
        let moved: Vec<String> = self
            .records
            .values()
            .filter(|current| current.path.starts_with(&source))
            .map(|current| current.id.clone())
            .collect();
        for id in moved {
            if let Some(current) = self.records.get_mut(&id) {
                self.ids_by_path.remove(&current.path);
                let below = current.path.strip_prefix(&source).unwrap_or(Path::new(""));
                current.path = if below.as_os_str().is_empty() {
                    destination.to_path_buf()
                } else {
                    destination.join(below)
                };
                self.ids_by_path.insert(current.path.clone(), id);
            }
        }
        // Recorded after each move so a later failure cannot lose a moved file's id.
        Ok(())
    }

    /// The companion files that move with the database at `source` to the vacant `destination`;
    /// none when `source` is not a database. Clears the way first (`clear_stale_companions`).
    fn companions_to_take(
        &self,
        source: &Path,
        destination: &Path,
    ) -> DriveResult<Vec<(&'static str, PathBuf)>> {
        if !crate::sheets::database::is_database(source) {
            return Ok(Vec::new());
        }
        self.clear_stale_companions(destination)?;
        Ok(crate::sheets::database::companions(source))
    }

    /// Trashes stale companion files at the vacant `destination` before a database arrives.
    /// They were left by a database moved or deleted outside the drive, and SQLite would replay
    /// a stale write-ahead log over the new one.
    fn clear_stale_companions(&self, destination: &Path) -> DriveResult<()> {
        let stale = crate::sheets::database::companions(destination);
        if stale.is_empty() {
            return Ok(());
        }
        let Some(trash) = &self.trash else {
            return refused(key::DRIVE_NO_TRASH);
        };
        for (_, file) in stale {
            trash(&file).map_err(|cause| io_error("move to the trash", &file, &cause))?;
        }
        Ok(())
    }

    /// Why `path` may not be changed, or `None`. A link leading outside the drive may be listed,
    /// but neither read nor changed.
    pub fn protection_of(&self, path: &Path) -> Option<Protection> {
        let Ok(canonical) = fs::canonicalize(path) else {
            return Some(Protection::Unavailable);
        };
        if !canonical.starts_with(&self.root) {
            return Some(Protection::Unavailable);
        }
        let mac = cfg!(target_os = "macos");
        let found = [
            path_protection(path, &self.root, &self.home, mac),
            path_protection(&canonical, &self.root, &self.home, mac),
        ];
        if found.contains(&Some(Protection::System)) {
            return Some(Protection::System);
        }
        found
            .contains(&Some(Protection::Location))
            .then_some(Protection::Location)
    }

    /// Why no new file may be created in `directory` (such as the archive Compress writes), or
    /// `None` when allowed. Fails when the folder's contents are protected.
    pub fn refusal_in(&self, directory: &Path) -> Option<Protection> {
        self.protection_of(directory)
            .filter(|protection| protection.protects_contents())
    }

    /// Refuses a change to the file itself: rename, move or trash.
    fn assert_mutable(&self, record: &Record) -> DriveResult<()> {
        match self.protection_of(&record.path) {
            Some(protection) => refused(protection.words()),
            None => Ok(()),
        }
    }

    /// Refuses a metadata change for a file whose contents are protected.
    fn assert_metadata_writable(&self, path: &Path) -> DriveResult<()> {
        match self.protection_of(path) {
            Some(protection) if protection.protects_contents() => refused(protection.words()),
            _ => Ok(()),
        }
    }

    /// The folder at `location`, failing when its contents are protected.
    fn require_writable_directory(&mut self, location: &Location) -> DriveResult<PathBuf> {
        let directory = self.require_directory(location)?;
        self.assert_metadata_writable(&directory)?;
        Ok(directory)
    }

    /// Copies files and folders from anywhere, the drive included, into `location`: folders
    /// recursively, links as links. A taken name is numbered ("stem 2.ext"); nothing is replaced.
    /// Copying a folder into itself or a descendant is refused. Every source is checked before
    /// copying starts. Returns the copies' ids in `sources` order.
    pub fn copy_in(
        &mut self,
        location: &Location,
        sources: &[PathBuf],
    ) -> DriveResult<Vec<String>> {
        let directory = self.require_writable_directory(location)?;
        let inside = fs::canonicalize(&directory)
            .map_err(|cause| io_error("resolve", &directory, &cause))?;
        let planned = sources
            .iter()
            .map(|source| Self::plan_copy(source, &inside))
            .collect::<DriveResult<Vec<_>>>()?;
        let mut ids = Vec::with_capacity(planned.len());
        let mut copied = Ok(());
        for (source, name, kind) in planned {
            let made = Self::place_copy(&directory, source, &name, kind).and_then(|destination| {
                if kind == CopyKind::File {
                    self.copy_companions(source, &destination)?;
                }
                self.remember(&destination)
                    .map_err(|cause| io_error("read", &destination, &cause))
            });
            match made {
                Ok(record) => ids.push(record.id),
                Err(error) => {
                    copied = Err(error);
                    break;
                }
            }
        }
        // Copies made before a failure stay and keep their ids.
        copied.map(|()| ids)
    }

    /// Copies a database's journal and write-ahead log beside `destination` so the copy holds
    /// all its data. Does nothing for other files.
    fn copy_companions(&self, source: &Path, destination: &Path) -> DriveResult<()> {
        if !crate::sheets::database::is_database(source) {
            return Ok(());
        }
        self.clear_stale_companions(destination)?;
        for (suffix, companion) in crate::sheets::database::copied_companions(source) {
            fs::copy(
                &companion,
                crate::sheets::database::beside(destination, suffix),
            )
            .map_err(|cause| io_error("copy", &companion, &cause))?;
        }
        Ok(())
    }

    /// What copying `source` into the resolved folder `inside` brings, and under which name;
    /// fails when it cannot be copied there.
    fn plan_copy<'a>(source: &'a Path, inside: &Path) -> DriveResult<(&'a Path, String, CopyKind)> {
        let name = source
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        require_name(&name)?;
        let stat =
            fs::symlink_metadata(source).map_err(|cause| io_error("read", source, &cause))?;
        let kind = if stat.file_type().is_symlink() {
            CopyKind::Link
        } else if stat.is_dir() {
            CopyKind::Folder
        } else if stat.is_file() {
            CopyKind::File
        } else {
            return Err(refusal_naming(key::DRIVE_NOT_A_FILE, name));
        };
        if kind == CopyKind::Folder {
            let folder =
                fs::canonicalize(source).map_err(|cause| io_error("resolve", source, &cause))?;
            if inside.starts_with(&folder) {
                return Err(refusal_naming(key::DRIVE_COPY_INTO_ITSELF, name));
            }
        }
        Ok((source, name, kind))
    }

    /// Copies `source`, a `kind`, into `directory` as `name`, numbered when taken. Returns the
    /// copy's path. A partial copy is removed on failure.
    fn place_copy(
        directory: &Path,
        source: &Path,
        name: &str,
        kind: CopyKind,
    ) -> DriveResult<PathBuf> {
        match kind {
            CopyKind::File => {
                let (destination, _) = Self::place_numbered(directory, name, |candidate| {
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(candidate)
                })?;
                if let Err(cause) = fs::copy(source, &destination) {
                    Self::take_away(&destination, |copy| fs::remove_file(copy));
                    return Err(io_error("copy", source, &cause));
                }
                Ok(destination)
            }
            CopyKind::Folder => {
                let (destination, ()) =
                    Self::place_numbered(directory, name, |candidate| fs::create_dir(candidate))?;
                if let Err(error) = copy_tree(source, &destination) {
                    Self::take_away(&destination, |copy| fs::remove_dir_all(copy));
                    return Err(error);
                }
                Ok(destination)
            }
            CopyKind::Link => {
                let target =
                    fs::read_link(source).map_err(|cause| io_error("read", source, &cause))?;
                let (destination, ()) = Self::place_numbered(directory, name, |candidate| {
                    make_link(&target, source, candidate)
                })?;
                Ok(destination)
            }
        }
    }

    /// Removes a partial copy with `remove`. A removal failure is only logged, since the original
    /// failure is what gets reported.
    fn take_away(copy: &Path, remove: impl FnOnce(&Path) -> io::Result<()>) {
        if let Err(cause) = remove(copy) {
            eprintln!("a failed copy was left at {}: {cause}", copy.display());
        }
    }

    /// Creates `name` in `directory` with `make`, trying "stem 2.ext", "stem 3.ext"… while `make`
    /// reports `AlreadyExists`. Never replaces anything. Returns the path and `make`'s result.
    fn place_numbered<T>(
        directory: &Path,
        name: &str,
        make: impl FnMut(&Path) -> io::Result<T>,
    ) -> DriveResult<(PathBuf, T)> {
        let extension = Path::new(name)
            .extension()
            .map(|extension| format!(".{}", extension.to_string_lossy()))
            .unwrap_or_default();
        let stem = &name[..name.len() - extension.len()];
        Self::place_numbered_parts(directory, (stem, &extension), make)
    }

    /// `place_numbered` with the name already split into stem and extension ("" for none).
    fn place_numbered_parts<T>(
        directory: &Path,
        (stem, extension): (&str, &str),
        mut make: impl FnMut(&Path) -> io::Result<T>,
    ) -> DriveResult<(PathBuf, T)> {
        let name = format!("{stem}{extension}");
        require_name(&name)?;
        for attempt in 1..=NUMBERED_NAME_ATTEMPTS {
            let candidate = if attempt == 1 {
                name.to_owned()
            } else {
                format!("{stem} {attempt}{extension}")
            };
            let destination = directory.join(candidate);
            match make(&destination) {
                Ok(made) => return Ok((destination, made)),
                Err(cause) if cause.kind() == io::ErrorKind::AlreadyExists => {}
                Err(cause) => return Err(io_error("create", &destination, &cause)),
            }
        }
        Err(refusal_naming(key::DRIVE_NAMES_TAKEN, &name))
    }

    /// Creates a new file named `name`, numbered when taken, and fills it with `write`. Never
    /// replaces a file. Returns its path.
    fn place_numbered_file(
        directory: &Path,
        name: &str,
        write: impl FnOnce(&mut fs::File) -> io::Result<()>,
    ) -> DriveResult<PathBuf> {
        let (destination, mut file) = Self::place_numbered(directory, name, |candidate| {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(candidate)
        })?;
        write(&mut file).map_err(|cause| io_error("write", &destination, &cause))?;
        Ok(destination)
    }

    /// Creates a folder named `title` in `location`. Returns its id.
    pub fn create_folder(&mut self, location: &Location, title: &str) -> DriveResult<String> {
        let directory = self.require_writable_directory(location)?;
        require_name(title)?;
        let destination = directory.join(title);
        fs::create_dir(&destination).map_err(|cause| {
            if cause.kind() == io::ErrorKind::AlreadyExists {
                refusal_naming(key::DRIVE_NAME_TAKEN, title)
            } else {
                io_error("create", &destination, &cause)
            }
        })?;
        let id = self
            .remember(&destination)
            .map_err(|cause| io_error("read", &destination, &cause))?
            .id;
        Ok(id)
    }

    /// Creates a folder named `title`, or "title 2", "title 3"… when taken, as Finder does; a
    /// dot is part of the name. Returns its id.
    pub fn create_numbered_folder(
        &mut self,
        location: &Location,
        title: &str,
    ) -> DriveResult<String> {
        let directory = self.require_writable_directory(location)?;
        let (destination, ()) = Self::place_numbered_parts(&directory, (title, ""), |candidate| {
            fs::create_dir(candidate)
        })?;
        let id = self
            .remember(&destination)
            .map_err(|cause| io_error("read", &destination, &cause))?
            .id;
        Ok(id)
    }

    /// Writes a new file named `name`, numbered when taken. Never replaces a file. Returns its
    /// id.
    pub fn create_file(
        &mut self,
        location: &Location,
        name: &str,
        bytes: &[u8],
    ) -> DriveResult<String> {
        use std::io::Write;
        let directory = self.require_writable_directory(location)?;
        let destination =
            Self::place_numbered_file(&directory, name, |file| file.write_all(bytes))?;
        let id = self
            .remember(&destination)
            .map_err(|cause| io_error("read", &destination, &cause))?
            .id;
        Ok(id)
    }

    /// Replaces a listed text file's contents, keeping its name and id. Writes beside the file
    /// and renames over it, so a crash leaves the old file intact.
    pub fn write_text(&mut self, id: &str, text: &str) -> DriveResult<()> {
        let record = self.require_record(id)?;
        let staged = record.path.with_extension("northroom-writing");
        fs::write(&staged, text).map_err(|cause| io_error("write", &staged, &cause))?;
        fs::rename(&staged, &record.path)
            .map_err(|cause| io_error("replace", &record.path, &cause))?;
        // The file has a new inode but is the same listed file.
        let stat = fs::symlink_metadata(&record.path)
            .map_err(|cause| io_error("read", &record.path, &cause))?;
        if let Some(saved) = self.records.get_mut(id) {
            (saved.device, saved.inode) = crate::drive::file_stat::identity(&record.path, &stat);
        }
        Ok(())
    }

    /// Renames a file in `location` to `title`. A page or terminal is renamed by its display
    /// name and keeps its kind.
    pub fn rename(&mut self, location: &Location, id: &str, title: &str) -> DriveResult<()> {
        let directory = self.require_directory(location)?;
        let record = self.require_child(id, &directory)?;
        require_name(title)?;
        let current = record
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let name = match named_file_extension(&current) {
            Some(extension) if !ends_with_ignoring_case(title, extension) => {
                format!("{title}{extension}")
            }
            _ => title.to_owned(),
        };
        self.move_record(&record, &directory.join(name))
    }

    /// Moves files from `source` into `destination`, each keeping its name.
    pub fn move_items(
        &mut self,
        source: &Location,
        ids: &[String],
        destination: &Location,
    ) -> DriveResult<()> {
        let directory = self.require_writable_directory(source)?;
        let target = self.require_writable_directory(destination)?;
        for id in ids {
            let record = self.require_child(id, &directory)?;
            let name = record.path.file_name().unwrap_or_default().to_owned();
            self.move_record(&record, &target.join(name))?;
        }
        Ok(())
    }

    /// Trashes a file in `location` and forgets it and its contents. Returns where it went and
    /// what `restore` needs.
    pub fn delete(&mut self, location: &Location, id: &str) -> DriveResult<Trashed> {
        let directory = self.require_directory(location)?;
        let record = self.require_child(id, &directory)?;
        self.trash_record(&record)
    }

    /// Trashes the file at `path` inside the drive, as `delete` does.
    pub fn trash_path(&mut self, path: &Path) -> DriveResult<Trashed> {
        let record = self.record_at(path)?;
        self.trash_record(&record)
    }

    fn trash_record(&mut self, record: &Record) -> DriveResult<Trashed> {
        self.assert_mutable(record)?;
        let Some(trash) = &self.trash else {
            return refused(key::DRIVE_NO_TRASH);
        };
        // Trash a database with its companion files; left behind, the next database of that
        // name would read them as its own.
        let companions = if crate::sheets::database::is_database(&record.path) {
            crate::sheets::database::companions(&record.path)
        } else {
            Vec::new()
        };
        let landed = trash(&record.path)
            .map_err(|cause| io_error("move to the trash", &record.path, &cause))?;
        let mut companions_landed = Vec::with_capacity(companions.len());
        for (suffix, companion) in companions {
            let landed = trash(&companion)
                .map_err(|cause| io_error("move to the trash", &companion, &cause))?;
            companions_landed.extend(landed.map(|landed| (suffix, landed)));
        }
        let gone: Vec<String> = self
            .records
            .values()
            .filter(|current| current.path.starts_with(&record.path))
            .map(|current| current.id.clone())
            .collect();
        let mut records = Vec::with_capacity(gone.len());
        for id in gone {
            if let Some(removed) = self.records.remove(&id) {
                self.ids_by_path.remove(&removed.path);
                records.push(removed);
            }
        }
        Ok(Trashed {
            landed,
            kept: Kept {
                path: record.path.clone(),
                records,
                companions: companions_landed,
            },
        })
    }

    /// Restores the trashed file at `landed` to `to` in the drive, with what the drive `kept`:
    /// the ids of it and its unchanged contents, and a database's companion files.
    pub fn restore(&mut self, landed: &Path, kept: &Kept, to: &Path) -> DriveResult<()> {
        self.require_vacant_place(to)?;
        if crate::sheets::database::is_database(landed) {
            self.clear_stale_companions(to)?;
        }
        fs::rename(landed, to).map_err(|cause| io_error("put back", landed, &cause))?;
        for (suffix, companion) in &kept.companions {
            fs::rename(companion, crate::sheets::database::beside(to, suffix))
                .map_err(|cause| io_error("put back", companion, &cause))?;
        }
        for record in &kept.records {
            let below = record
                .path
                .strip_prefix(&kept.path)
                .unwrap_or(Path::new(""));
            let path = if below.as_os_str().is_empty() {
                to.to_path_buf()
            } else {
                to.join(below)
            };
            self.readopt(record, path);
        }
        self.remember(to)
            .map_err(|cause| io_error("read", to, &cause))?;
        Ok(())
    }

    /// Reattaches `record` to the file now at `path` when it is the same file and the id is
    /// free; otherwise it gets a new record when next listed.
    fn readopt(&mut self, record: &Record, path: PathBuf) {
        let Ok(stat) = fs::symlink_metadata(&path) else {
            return;
        };
        let (device, inode) = crate::drive::file_stat::identity(&path, &stat);
        if record.device != device || record.inode != inode || self.records.contains_key(&record.id)
        {
            return;
        }
        if let Some(previous) = self.ids_by_path.remove(&path) {
            self.records.remove(&previous);
        }
        self.ids_by_path.insert(path.clone(), record.id.clone());
        self.records.insert(
            record.id.clone(),
            Record {
                path,
                ..record.clone()
            },
        );
    }

    /// Moves `from` to `to` within the drive, keeping its id and its contents' ids, as a drag or
    /// rename does.
    pub fn move_path(&mut self, from: &Path, to: &Path) -> DriveResult<()> {
        let record = self.record_at(from)?;
        if let Some(parent) = from.parent() {
            self.assert_metadata_writable(parent)?;
        }
        self.require_vacant_place(to)?;
        self.move_record(&record, to)
    }

    /// The record for `path`, created if the drive has none.
    fn record_at(&mut self, path: &Path) -> DriveResult<Record> {
        self.assert_contained(path)?;
        self.remember(path)
            .map_err(|cause| io_error("read", path, &cause))
    }

    /// Fails unless `path` is vacant and inside a drive folder whose contents may change.
    fn require_vacant_place(&self, path: &Path) -> DriveResult<()> {
        self.assert_contained(path)?;
        let Some(parent) = path.parent() else {
            return refused(key::DRIVE_OUTSIDE);
        };
        self.require_folder(parent)?;
        self.assert_metadata_writable(parent)?;
        Self::require_vacant(path)
    }

    fn assert_contained(&self, path: &Path) -> DriveResult<()> {
        if path.starts_with(&self.root) {
            Ok(())
        } else {
            refused(key::DRIVE_OUTSIDE)
        }
    }

    /// The record for `path`: the one this run assigned, the same file's record from before a
    /// rename, or a new one for an unseen file.
    fn remember(&mut self, path: &Path) -> Result<Record, io::Error> {
        let stat = fs::symlink_metadata(path)?;
        let (device, inode) = crate::drive::file_stat::identity(path, &stat);
        let previous = self
            .ids_by_path
            .get(path)
            .and_then(|id| self.records.get(id))
            .cloned();
        if let Some(previous) = &previous {
            if previous.device == device && previous.inode == inode {
                return Ok(previous.clone());
            }
            self.records.remove(&previous.id);
        }
        let record = match self.renamed(&device, &inode) {
            Some(mut renamed) => {
                self.ids_by_path.remove(&renamed.path);
                renamed.path = path.to_path_buf();
                renamed
            }
            None => Record {
                id: self.new_id(path, &device, &inode),
                path: path.to_path_buf(),
                device,
                inode,
            },
        };
        self.ids_by_path
            .insert(path.to_path_buf(), record.id.clone());
        self.records.insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// The record for this identity when its known path no longer holds it, meaning the file
    /// was renamed or moved by anything.
    fn renamed(&self, device: &str, inode: &str) -> Option<Record> {
        if device.is_empty() || inode.is_empty() {
            return None;
        }
        let known = self.records.get(&identity_id(device, inode))?;
        let still_there = fs::symlink_metadata(&known.path).is_ok_and(|stat| {
            crate::drive::file_stat::identity(&known.path, &stat)
                == (device.to_owned(), inode.to_owned())
        });
        (known.device == device && known.inode == inode && !still_there).then(|| known.clone())
    }

    /// The id for an unseen file: its identity, so the next run assigns the same one. Numbered
    /// when another file holds that id (a hard link, or a file rewritten in place that kept its
    /// id), or when the system gives no identity.
    fn new_id(&self, path: &Path, device: &str, inode: &str) -> String {
        if path == self.root {
            return DRIVE_ROOT_ID.to_owned();
        }
        let base = identity_id(device, inode);
        if !self.records.contains_key(&base) {
            return base;
        }
        (2..)
            .map(|number| format!("{base}-{number}"))
            .find(|id| !self.records.contains_key(id))
            .expect("an unused id")
    }

    /// The record for `id`, failing if its file is gone or replaced since listing.
    fn require_record(&mut self, id: &str) -> DriveResult<Record> {
        if id == DRIVE_ROOT_ID {
            let root = self.root.clone();
            return self
                .remember(&root)
                .map_err(|cause| io_error("read", &root, &cause));
        }
        self.listed_record(id)
    }

    /// The record for listed `id` (not the root), failing if its file is gone or replaced since
    /// listing. Read-only.
    fn listed_record(&self, id: &str) -> DriveResult<Record> {
        let Some(record) = self.records.get(id).cloned() else {
            return refused(key::DRIVE_FILE_GONE);
        };
        let stat = fs::symlink_metadata(&record.path)
            .map_err(|_| DriveError::Refused(Message::new(key::DRIVE_FILE_GONE)))?;
        let (device, inode) = crate::drive::file_stat::identity(&record.path, &stat);
        if record.device != device || record.inode != inode {
            return refused(key::DRIVE_FILE_REPLACED);
        }
        let canonical = fs::canonicalize(&record.path)
            .map_err(|cause| io_error("resolve", &record.path, &cause))?;
        self.assert_contained(&canonical)?;
        Ok(record)
    }

    /// The path of `location`. A linked folder keeps its link path, but the target must be a
    /// folder inside the drive.
    pub fn require_directory(&mut self, location: &Location) -> DriveResult<PathBuf> {
        let record = self.require_record(location.folder_id.as_deref().unwrap_or(DRIVE_ROOT_ID))?;
        self.require_folder(&record.path)?;
        Ok(record.path)
    }

    /// Fails unless `path` resolves to a folder inside the drive.
    fn require_folder(&self, path: &Path) -> DriveResult<()> {
        let canonical =
            fs::canonicalize(path).map_err(|cause| io_error("resolve", path, &cause))?;
        self.assert_contained(&canonical)?;
        if !canonical.is_dir() {
            return refused(key::DRIVE_NOT_A_FOLDER);
        }
        Ok(())
    }

    /// The tab title for `location`: the drive label for the root, else the folder name. Reads
    /// the disk without changing state.
    pub fn folder_title(&self, location: &Location) -> DriveResult<String> {
        let id = location.folder_id.as_deref().unwrap_or(DRIVE_ROOT_ID);
        if id == DRIVE_ROOT_ID {
            return Ok(self.root_label.clone());
        }
        let record = self.listed_record(id)?;
        self.require_folder(&record.path)?;
        Ok(format_title(
            &record
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        )
        .to_owned())
    }

    /// The location for `path`, when it is a folder inside the drive.
    pub fn location_at(&mut self, path: &Path) -> DriveResult<Location> {
        let canonical =
            fs::canonicalize(path).map_err(|cause| io_error("resolve", path, &cause))?;
        self.assert_contained(&canonical)?;
        if !canonical.is_dir() {
            return refused(key::DRIVE_NOT_A_FOLDER);
        }
        if canonical == self.root {
            return Ok(Location::ROOT);
        }
        let record = self
            .remember(&canonical)
            .map_err(|cause| io_error("read", &canonical, &cause))?;
        Ok(Location {
            folder_id: Some(record.id),
        })
    }

    /// The id for `path`, when it is inside the drive.
    pub fn id_at(&mut self, path: &Path) -> DriveResult<String> {
        let canonical =
            fs::canonicalize(path).map_err(|cause| io_error("resolve", path, &cause))?;
        self.assert_contained(&canonical)?;
        if canonical.is_dir() {
            return Err(refusal_naming(
                key::DRIVE_NOT_A_FILE,
                canonical.to_string_lossy().into_owned(),
            ));
        }
        Ok(self
            .remember(&canonical)
            .map_err(|cause| io_error("read", &canonical, &cause))?
            .id)
    }

    /// The current path for `id`.
    pub fn path_of(&mut self, id: &str) -> DriveResult<PathBuf> {
        Ok(self.require_record(id)?.path)
    }

    /// The path `id` was listed at, without checking the file still exists. A cheap peek before
    /// `path_of`.
    pub fn listed_path(&self, id: &str) -> Option<&Path> {
        if id == DRIVE_ROOT_ID {
            return Some(&self.root);
        }
        self.records.get(id).map(|record| record.path.as_path())
    }

    /// Where `id` is, for lists of files from across the drive: its card name, its folder, and
    /// the folders from the root down.
    pub fn place_of(&mut self, id: &str) -> DriveResult<Placed> {
        let path = self.path_of(id)?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let directory = path.parent().unwrap_or(&self.root).to_path_buf();
        let folder = self.location_at(&directory)?;
        let folders = self.path(&folder)?;
        Ok(Placed {
            title: format_title(&name).to_owned(),
            folder,
            folders,
        })
    }

    fn read_entry(&self, record: &Record) -> Result<Entry, io::Error> {
        let stat = fs::metadata(&record.path)?;
        let name = record
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = record
            .path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let shape = Shape::of(&stat, &record.path, &extension);
        let kind = match shape {
            Shape::Folder => EntryKind::Folder,
            Shape::File => file_kind(&record.path, &extension),
        };
        Ok(Entry {
            id: record.id.clone(),
            kind,
            title: if record.path == self.root {
                self.root_label.clone()
            } else {
                format_title(&name).to_owned()
            },
            // A folder listed as a file (an application, a document package) has no single
            // size.
            size_bytes: stat.is_file().then_some(stat.len()),
            modified_seconds: crate::drive::file_stat::modified(&stat).0,
            protection: self.protection_of(&record.path),
            family: match kind {
                EntryKind::File => FileFamily::of_extension(&extension),
                _ => FileFamily::Other,
            },
            created_seconds: crate::drive::file_stat::created(&stat),
            accessed_seconds: crate::drive::file_stat::accessed(&stat).map(|(seconds, _)| seconds),
            changed_seconds: crate::drive::file_stat::changed(&stat).map(|(seconds, _)| seconds),
            extension: (shape != Shape::Folder)
                .then(|| {
                    record
                        .path
                        .extension()
                        .map(|written| written.to_string_lossy().into_owned())
                })
                .flatten()
                .filter(|written| !written.is_empty()),
            location: record
                .path
                .parent()
                .map(|parent| parent.to_string_lossy().into_owned())
                .unwrap_or_default(),
            owner: crate::drive::file_stat::owner(&stat),
            permissions: crate::drive::file_stat::permissions(&stat),
            indexed: if self.index_wanted {
                self.index_of(&record.path, kind)
            } else {
                None
            },
            kind_description: match shape {
                Shape::Folder => None,
                Shape::File => self.kind_description_at(&record.path),
            },
        })
    }

    /// The contents of `location` in name order, from a single read of the folder.
    pub fn all_contents(&mut self, location: &Location) -> DriveResult<Vec<Entry>> {
        let directory = self.require_directory(location)?;
        // Read only this directory; never walk a subtree to count or size a folder.
        let mut names: Vec<String> = fs::read_dir(&directory)
            .map_err(|cause| io_error("list", &directory, &cause))?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_name_listed(&directory, name, self.hidden_shown))
            .collect();
        names.sort_by(|left, right| compare_names(left, right));
        // Assign records first, in order, since only this changes known ids. Then read each
        // file's details, which hit the disk and platform readers once per file, in parallel.
        let mut records = Vec::with_capacity(names.len());
        for name in &names {
            let path = directory.join(name);
            match self.remember(&path) {
                Ok(record) => records.push(record),
                Err(cause) if is_skippable(&cause) => {}
                Err(cause) => return Err(io_error("read", &path, &cause)),
            }
        }
        let this = &*self;
        let read = each_on_threads(&records, LISTING_SPREAD, |record| this.read_entry(record));
        let mut entries = Vec::with_capacity(records.len());
        for (record, read) in records.iter().zip(read) {
            match read {
                Ok(entry) => entries.push(entry),
                Err(cause) if is_skippable(&cause) => {}
                Err(cause) => return Err(io_error("read", &record.path, &cause)),
            }
        }
        Ok(entries)
    }

    /// The id and kind of the media file `directory` lists under exactly `name`; `None` if there
    /// is none or it is not media. Only names are read, so a page with pictures costs the same in
    /// a folder of any size.
    pub fn media_named(&mut self, directory: &Path, name: &str) -> Option<(String, EntryKind)> {
        let extension = Path::new(name)
            .extension()?
            .to_string_lossy()
            .to_lowercase();
        if !is_name_listed(directory, name, self.hidden_shown)
            || !matches!(
                media_of_extension(&extension),
                Some(Media::Image | Media::Video | Media::Audio)
            )
        {
            return None;
        }
        // Match the folder's exact spelling; a case-insensitive volume would accept others.
        let listed = fs::read_dir(directory)
            .ok()?
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy() == name);
        let path = directory.join(name);
        if !listed || !fs::metadata(&path).ok()?.is_file() {
            return None;
        }
        let kind = media_kind(&path);
        let known = self.ids_by_path.contains_key(&path);
        let record = self.remember(&path).ok()?;
        if !known {
            // A file first seen here keeps its id, as in a listing.
        }
        Some((record.id, kind))
    }

    /// The folders from the root down to `location`.
    pub fn path(&mut self, location: &Location) -> DriveResult<Vec<Crumb>> {
        let mut directory = self.require_directory(location)?;
        let mut crumbs = Vec::new();
        loop {
            let record = self
                .remember(&directory)
                .map_err(|cause| io_error("read", &directory, &cause))?;
            let is_root = directory == self.root;
            crumbs.push(Crumb {
                location: Location {
                    folder_id: (!is_root).then(|| record.id.clone()),
                },
                title: if is_root {
                    self.root_label.clone()
                } else {
                    format_title(&directory.file_name().unwrap_or_default().to_string_lossy())
                        .to_owned()
                },
            });
            if is_root {
                break;
            }
            let Some(parent) = directory.parent() else {
                break;
            };
            directory = parent.to_path_buf();
        }
        crumbs.reverse();
        Ok(crumbs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protection_is_said_in_the_interface_language() {
        let say =
            |protection: Protection, language| crate::strings::text(language, protection.words());
        assert_eq!(
            say(Protection::System, "en"),
            "This system item is protected. You can browse it, but cannot change it in Northroom."
        );
        assert_eq!(
            say(Protection::Location, "en"),
            "This standard location cannot be renamed, moved, or sent to Trash. You can organize its contents."
        );
        assert_eq!(
            say(Protection::Unavailable, "en"),
            "Northroom couldn’t verify this item’s protection. Refresh the folder before changing it."
        );
    }

    #[test]
    fn a_text_files_name_says_how_it_is_shown() {
        assert_eq!(text_form("main.rs"), TextForm::Source);
        assert_eq!(text_form("README"), TextForm::Source);
        assert_eq!(text_form("notes.TXT"), TextForm::PlainPage);
        assert_eq!(text_form("a.b.text"), TextForm::PlainPage);
        assert_eq!(text_form("rows.csv"), TextForm::Table);
        assert_eq!(text_form("rows.tsv"), TextForm::Table);
        assert_eq!(text_form(".gitignore"), TextForm::Source);
        assert_eq!(name_without_extension("A long name.txt"), "A long name");
        assert_eq!(name_without_extension(".gitignore"), ".gitignore");
        assert_eq!(name_without_extension("README"), "README");
    }

    #[test]
    fn only_small_valid_text_without_a_nul_reads_as_text() {
        let folder = tempfile::tempdir().expect("folder");
        let write = |name: &str, bytes: &[u8]| {
            let path = folder.path().join(name);
            std::fs::write(&path, bytes).expect("write");
            path
        };
        assert_eq!(
            read_text_file(&write("main.rs", b"fn main() {}\n")).as_deref(),
            Some("fn main() {}\n")
        );
        assert_eq!(read_text_file(&write("nul.rs", b"fn\0main")), None);
        assert_eq!(
            read_text_file(&write("latin.rs", &[b'a', 0xE9, b'b'])),
            None,
            "not UTF-8"
        );
        let large = vec![b'a'; (TEXT_PREVIEW_MAX_BYTES + 1) as usize];
        assert_eq!(read_text_file(&write("large.rs", &large)), None);
        let most = vec![b'a'; TEXT_PREVIEW_MAX_BYTES as usize];
        assert!(read_text_file(&write("most.rs", &most)).is_some());
    }

    #[test]
    fn a_checks_trash_keeps_what_it_is_sent_in_its_own_folder() {
        let root = tempfile::tempdir().expect("root");
        let kept = root.path().join("kept");
        let trash = trash_into(kept.clone());
        for _ in 0..2 {
            let file = root.path().join("note.txt");
            fs::write(&file, b"x").expect("file");
            trash(&file).expect("trashed");
            assert!(!file.exists());
        }
        assert!(kept.join("note.txt").exists() && kept.join("note.txt 2").exists());
    }

    fn drive() -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        for name in [
            "AGENTS.md",
            "Claude Code.nterm",
            "clip.MP4",
            ".hidden",
            ".DS_Store",
            "notes.txt",
        ] {
            fs::write(root.path().join(name), b"x").expect("file");
        }
        fs::create_dir(root.path().join("Northroom")).expect("folder");
        fs::create_dir(root.path().join("Cut.timeline")).expect("timeline");
        fs::create_dir(root.path().join("Northroom").join("Inner")).expect("inner");
        let workspace = Workspace::open(root.path(), metadata.path()).expect("open");
        (root, metadata, workspace)
    }

    #[test]
    fn an_id_follows_a_file_renamed_outside_and_nothing_is_written() {
        let (root, metadata, mut workspace) = drive();
        let first = workspace.all_contents(&Location::ROOT).expect("contents");
        let notes = first
            .iter()
            .find(|entry| entry.title == "notes.txt")
            .expect("notes")
            .id
            .clone();
        // Renamed by an agent, not through the drive.
        fs::rename(root.path().join("notes.txt"), root.path().join("ideas.txt")).expect("rename");
        let after = workspace.all_contents(&Location::ROOT).expect("contents");
        let ideas = after
            .iter()
            .find(|entry| entry.title == "ideas.txt")
            .expect("ideas");
        assert_eq!(ideas.id, notes);
        assert!(after.iter().all(|entry| entry.title != "notes.txt"));
        // The same id after a restart, with nothing stored.
        let mut reopened = Workspace::open(root.path(), metadata.path()).expect("reopen");
        let again = reopened.all_contents(&Location::ROOT).expect("contents");
        assert!(again
            .iter()
            .any(|entry| entry.id == notes && entry.title == "ideas.txt"));
        assert_eq!(fs::read_dir(metadata.path()).expect("metadata").count(), 0);
    }

    #[test]
    fn an_earlier_versions_catalog_is_deleted_and_what_else_is_kept_stays() {
        let root = tempfile::tempdir().expect("root");
        let metadata = tempfile::tempdir().expect("metadata");
        for name in RETIRED_FILES.into_iter().chain(["view.json"]) {
            fs::write(metadata.path().join(name), b"{}").expect("kept");
        }
        Workspace::open(root.path(), metadata.path()).expect("open");
        let left: Vec<String> = fs::read_dir(metadata.path())
            .expect("metadata")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(left, ["view.json"]);
    }

    #[test]
    fn lists_a_folder_by_utf16_code_unit() {
        let (_root, _metadata, mut workspace) = drive();
        let entries = workspace.all_contents(&Location::ROOT).expect("contents");
        let listed: Vec<(&str, EntryKind)> = entries
            .iter()
            .map(|entry| (entry.title.as_str(), entry.kind))
            .collect();
        assert_eq!(
            listed,
            [
                ("AGENTS", EntryKind::Page),
                ("Claude Code", EntryKind::Terminal),
                ("Cut.timeline", EntryKind::Folder),
                ("Northroom", EntryKind::Folder),
                ("clip.MP4", EntryKind::Video),
                ("notes.txt", EntryKind::File),
            ]
        );
        assert_eq!(entries[3].size_bytes, None);
        assert_eq!(entries[5].size_bytes, Some(1));
    }

    #[test]
    fn shows_hidden_files_when_asked_but_never_finder_bookkeeping() {
        let (_root, _metadata, mut workspace) = drive();
        workspace.set_hidden_shown(true);
        let titles: Vec<String> = workspace
            .all_contents(&Location::ROOT)
            .expect("contents")
            .into_iter()
            .map(|entry| entry.title)
            .collect();
        assert!(titles.contains(&".hidden".to_owned()));
        assert!(!titles.contains(&".DS_Store".to_owned()));
    }

    /// A database at `file` with one table and one row, as an agent makes one.
    fn make_database(file: &Path) {
        let run = crate::sheets::database::asked(&serde_json::json!([
            ["CREATE TABLE people (name, age)", []],
            ["INSERT INTO people VALUES ('Ada', 36)", []]
        ]))
        .expect("a call");
        crate::sheets::database::ask(file, &run, crate::sheets::database::Access::Creates)
            .expect("a database");
    }

    #[test]
    fn a_database_is_read_as_one_whatever_its_name_and_is_no_text() {
        let folder = tempfile::tempdir().expect("folder");
        for name in ["people.sqlite", "people.db", "People"] {
            let file = folder.path().join(name);
            make_database(&file);
            assert_eq!(
                read_file(&file),
                FileRead::Database("name  age\nAda   36".to_owned()),
                "{name}"
            );
            assert_eq!(read_text_file(&file), None);
            assert!(!read_file(&file).shows_system_thumbnail());
        }
        // A file with a database name but other bytes is not a database.
        let words = folder.path().join("words.sqlite");
        fs::write(&words, "plain words").expect("file");
        assert_eq!(read_file(&words), FileRead::Text("plain words".to_owned()));
    }

    /// The names in the drive's root that are `name` or one of its companions.
    fn with_companions(root: &Path, name: &str) -> Vec<String> {
        let mut found: Vec<String> = fs::read_dir(root)
            .expect("root")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|listed| listed.starts_with(name))
            .collect();
        found.sort();
        found
    }

    #[test]
    fn a_database_takes_its_companion_files_where_it_goes() {
        let (root, _metadata, mut workspace) = drive();
        let bin = tempfile::tempdir().expect("bin");
        workspace.set_trash(trash_into(bin.path().to_path_buf()));
        make_database(&root.path().join("shop.sqlite"));
        for suffix in ["-wal", "-shm"] {
            fs::write(root.path().join(format!("shop.sqlite{suffix}")), suffix).expect("file");
        }
        // Left by a database of the new name deleted outside the drive.
        fs::write(root.path().join("store.sqlite-wal"), "stale").expect("file");
        let id = id_of(&mut workspace, &Location::ROOT, "shop.sqlite");
        workspace
            .rename(&Location::ROOT, &id, "store.sqlite")
            .expect("rename");
        assert!(with_companions(root.path(), "shop.sqlite").is_empty());
        assert_eq!(
            with_companions(root.path(), "store.sqlite"),
            ["store.sqlite", "store.sqlite-shm", "store.sqlite-wal"]
        );
        // Its own log moved with it; the stale one was trashed.
        assert_eq!(
            fs::read_to_string(root.path().join("store.sqlite-wal")).expect("log"),
            "-wal"
        );
        assert_eq!(
            fs::read_to_string(bin.path().join("store.sqlite-wal")).expect("stale"),
            "stale"
        );
        // Into a folder, then back by path as a drag's undo does.
        let folder = Location {
            folder_id: Some(id_of(&mut workspace, &Location::ROOT, "Northroom")),
        };
        workspace
            .move_items(&Location::ROOT, std::slice::from_ref(&id), &folder)
            .expect("move");
        // The drive's paths start from the resolved root.
        let home = fs::canonicalize(root.path()).expect("root");
        let inside = home.join("Northroom");
        assert!(with_companions(root.path(), "store.sqlite").is_empty());
        assert_eq!(with_companions(&inside, "store.sqlite").len(), 3);
        workspace
            .move_path(&inside.join("store.sqlite"), &home.join("back.sqlite"))
            .expect("move back");
        assert_eq!(with_companions(root.path(), "back.sqlite").len(), 3);
        assert_eq!(
            workspace.path_of(&id).expect("path"),
            home.join("back.sqlite")
        );
        // A non-database moves alone, whatever lies beside it.
        fs::write(root.path().join("notes.txt-wal"), "x").expect("file");
        let notes = id_of(&mut workspace, &Location::ROOT, "notes.txt");
        workspace
            .rename(&Location::ROOT, &notes, "words.txt")
            .expect("rename");
        assert!(root.path().join("notes.txt-wal").exists());
        assert!(!root.path().join("words.txt-wal").exists());
    }

    #[test]
    fn a_database_goes_to_the_trash_comes_back_and_is_copied_with_its_companions() {
        let (root, _metadata, mut workspace) = drive();
        let bin = tempfile::tempdir().expect("bin");
        workspace.set_trash(trash_into(bin.path().to_path_buf()));
        let file = fs::canonicalize(root.path())
            .expect("root")
            .join("shop.sqlite");
        make_database(&file);
        for suffix in ["-journal", "-wal", "-shm"] {
            fs::write(root.path().join(format!("shop.sqlite{suffix}")), suffix).expect("file");
        }
        // A copy takes the journal and log; SQLite rebuilds the log index.
        let copies = workspace
            .copy_in(&Location::ROOT, std::slice::from_ref(&file))
            .expect("copy");
        assert_eq!(copies.len(), 1);
        assert_eq!(
            with_companions(root.path(), "shop 2.sqlite"),
            [
                "shop 2.sqlite",
                "shop 2.sqlite-journal",
                "shop 2.sqlite-wal"
            ]
        );
        // Everything is trashed, so a later database of that name finds nothing of this one.
        let id = id_of(&mut workspace, &Location::ROOT, "shop.sqlite");
        let trashed = workspace.delete(&Location::ROOT, &id).expect("delete");
        assert!(with_companions(root.path(), "shop.sqlite").is_empty());
        assert_eq!(with_companions(bin.path(), "shop.sqlite").len(), 4);
        // Restored, as undo does, it is complete again.
        let landed = trashed.landed.expect("where it landed");
        workspace
            .restore(&landed, &trashed.kept, &file)
            .expect("restore");
        assert_eq!(
            with_companions(root.path(), "shop.sqlite"),
            [
                "shop.sqlite",
                "shop.sqlite-journal",
                "shop.sqlite-shm",
                "shop.sqlite-wal"
            ]
        );
        assert!(with_companions(bin.path(), "shop.sqlite").is_empty());
        assert_eq!(workspace.path_of(&id).expect("path"), file);
    }

    #[test]
    fn a_database_s_companion_files_are_hidden_files() {
        let (root, _metadata, mut workspace) = drive();
        make_database(&root.path().join("shop.sqlite"));
        for name in [
            "shop.sqlite-journal",
            "shop.sqlite-wal",
            "shop.sqlite-shm",
            "notes.txt-wal",
        ] {
            fs::write(root.path().join(name), b"x").expect("file");
        }
        let titles = |workspace: &mut Workspace| -> Vec<String> {
            workspace
                .all_contents(&Location::ROOT)
                .expect("contents")
                .into_iter()
                .map(|entry| entry.title)
                .filter(|title| title.contains("-") || title.starts_with("shop"))
                .collect()
        };
        // Beside a non-database, a companion-like name is an ordinary file.
        assert_eq!(titles(&mut workspace), ["notes.txt-wal", "shop.sqlite"]);
        workspace.set_hidden_shown(true);
        assert_eq!(
            titles(&mut workspace),
            [
                "notes.txt-wal",
                "shop.sqlite",
                "shop.sqlite-journal",
                "shop.sqlite-shm",
                "shop.sqlite-wal"
            ]
        );
    }

    #[test]
    fn keeps_ids_across_listings_and_restarts_and_renews_them_for_replaced_files() {
        let (root, metadata, mut workspace) = drive();
        let first = workspace.all_contents(&Location::ROOT).expect("contents");
        let again = workspace.all_contents(&Location::ROOT).expect("contents");
        assert_eq!(first[0].id, again[0].id);
        drop(workspace);

        let mut reopened = Workspace::open(root.path(), metadata.path()).expect("reopen");
        let after_restart = reopened.all_contents(&Location::ROOT).expect("contents");
        assert_eq!(first[0].id, after_restart[0].id);

        // Keep the old file's inode in use so the new file cannot be given the same one.
        fs::rename(
            root.path().join("notes.txt"),
            root.path().join("notes-old.txt"),
        )
        .expect("move away");
        fs::write(root.path().join("notes.txt"), b"other").expect("recreate");
        let replaced = reopened.all_contents(&Location::ROOT).expect("contents");
        let old = first
            .iter()
            .find(|entry| entry.title == "notes.txt")
            .expect("old");
        let new = replaced
            .iter()
            .find(|entry| entry.title == "notes.txt")
            .expect("new");
        assert_ne!(old.id, new.id);
    }

    #[test]
    fn walks_the_path_from_the_drive_down() {
        let (_root, _metadata, mut workspace) = drive();
        let top = workspace.all_contents(&Location::ROOT).expect("contents");
        let folder = top
            .iter()
            .find(|entry| entry.title == "Northroom")
            .expect("folder");
        let location = Location {
            folder_id: Some(folder.id.clone()),
        };
        let inner = workspace.all_contents(&location).expect("inner contents");
        let inner_location = Location {
            folder_id: Some(inner[0].id.clone()),
        };
        let crumbs = workspace.path(&inner_location).expect("path");
        let titles: Vec<&str> = crumbs.iter().map(|crumb| crumb.title.as_str()).collect();
        assert_eq!(titles.len(), 3);
        assert_eq!(&titles[1..], ["Northroom", "Inner"]);
        assert_eq!(crumbs[0].location, Location::ROOT);
        assert_eq!(crumbs[2].location, inner_location);
    }

    #[test]
    fn names_a_folder_by_its_own_name_and_the_drive_by_its_label() {
        let (root, _metadata, mut workspace) = drive();
        let top = workspace.all_contents(&Location::ROOT).expect("contents");
        let folder = top
            .iter()
            .find(|entry| entry.title == "Northroom")
            .expect("folder");
        let location = Location {
            folder_id: Some(folder.id.clone()),
        };
        let inner = workspace.all_contents(&location).expect("inner contents");
        let inner_location = Location {
            folder_id: Some(inner[0].id.clone()),
        };
        // Takes `&self`, so it cannot record folders.
        let label = workspace.root_label().to_owned();
        assert_eq!(
            workspace.folder_title(&Location::ROOT).expect("drive"),
            label
        );
        assert_eq!(
            workspace.folder_title(&location).expect("folder"),
            "Northroom"
        );
        assert_eq!(
            workspace.folder_title(&inner_location).expect("inner"),
            "Inner"
        );
        fs::remove_dir_all(root.path().join("Northroom")).expect("remove");
        assert!(workspace.folder_title(&inner_location).is_err());
    }

    fn id_of(workspace: &mut Workspace, location: &Location, title: &str) -> String {
        workspace
            .all_contents(location)
            .expect("contents")
            .into_iter()
            .find(|entry| entry.title == title)
            .unwrap_or_else(|| panic!("no {title}"))
            .id
    }

    #[test]
    fn creates_folders_and_numbers_new_files_instead_of_replacing() {
        let (root, _metadata, mut workspace) = drive();
        let folder = workspace
            .create_folder(&Location::ROOT, "Shots")
            .expect("create");
        assert!(root.path().join("Shots").is_dir());
        assert_eq!(id_of(&mut workspace, &Location::ROOT, "Shots"), folder);
        let taken = workspace
            .create_folder(&Location::ROOT, "Shots")
            .expect_err("taken");
        assert_eq!(
            taken.to_string(),
            "A file named “Shots” already exists",
            "agents and the log read English"
        );
        assert_eq!(taken.say("en"), "A file named “Shots” already exists");
        let path = workspace
            .create_folder(&Location::ROOT, "a/b")
            .expect_err("a path");
        assert_eq!(path.say("en"), "A filename cannot contain a path");
        assert!(workspace.create_folder(&Location::ROOT, "..").is_err());

        workspace
            .create_file(&Location::ROOT, "notes.txt", b"second")
            .expect("numbered");
        workspace
            .create_file(&Location::ROOT, "notes.txt", b"third")
            .expect("numbered again");
        assert_eq!(
            fs::read(root.path().join("notes.txt")).expect("first"),
            b"x"
        );
        assert_eq!(
            fs::read(root.path().join("notes 2.txt")).expect("second"),
            b"second"
        );
        assert_eq!(
            fs::read(root.path().join("notes 3.txt")).expect("third"),
            b"third"
        );
    }

    #[test]
    fn a_document_package_is_read_as_the_single_file_finder_shows() {
        let root = tempfile::tempdir().expect("root");
        let package = root.path().join("Letter.rtfd");
        std::fs::create_dir(&package).expect("package");
        std::fs::write(package.join("TXT.rtf"), b"{\\rtf1 A}").expect("its text");
        // A package the app draws shows as pages, on a Mac where it is a package.
        let drawn = if cfg!(target_os = "macos") {
            FileRead::Deck
        } else {
            FileRead::External
        };
        assert_eq!(read_file(&package), drawn);
        assert!(
            is_package(&package, "rtfd") == cfg!(target_os = "macos"),
            "a package only on a Mac"
        );
        // A plain folder with a package name but no package contents stays a folder.
        let keys = root.path().join("deploy.key");
        std::fs::create_dir(&keys).expect("folder");
        std::fs::write(keys.join("server.pem"), b"pem").expect("a key");
        assert!(!is_package(&keys, "key"));
    }

    #[test]
    fn a_file_is_read_as_what_its_name_and_bytes_make_it() {
        let root = tempfile::tempdir().expect("root");
        let write = |name: &str, bytes: &[u8]| {
            fs::write(root.path().join(name), bytes).expect("write");
            root.path().join(name)
        };
        assert_eq!(
            read_text_file(&write("notes.txt", b"plain text\n")).as_deref(),
            Some("plain text\n")
        );
        assert_eq!(
            read_text_file(&write("Makefile", b"all:\n")).as_deref(),
            Some("all:\n")
        );
        assert_eq!(
            read_text_file(&write("empty.log", b"")).as_deref(),
            Some("")
        );
        assert_eq!(read_text_file(&write("binary.dat", b"a\0b")), None);
        assert_eq!(
            read_text_file(&write("latin.txt", &[0xe9, 0x74, 0xe9])),
            None
        );
        assert_eq!(read_text_file(&write("scene.glb", b"glTF")), None);
        assert_eq!(read_text_file(&write("book.XLSX", b"text")), None);
        assert_eq!(
            media_of_extension("ts"),
            None,
            "TypeScript, not a transport stream"
        );
        assert_eq!(media_of_extension("m2ts"), Some(Media::Video));
        assert_eq!(media_of_extension("opus"), Some(Media::Audio));
        // A still is a picture where the platform decodes it: HEIC on a Mac, on Windows with
        // the HEIF and HEVC extensions, never on Linux.
        let heic = if decodes_still("heic") {
            EntryKind::Image
        } else {
            EntryKind::File
        };
        assert_eq!(decodes_still("heic"), phone_still_decodes_here("heic"));
        assert_eq!(media_kind(Path::new("photo.HEIC")), heic);
        assert_eq!(media_kind(Path::new("scan.tiff")), EntryKind::Image);
        assert_eq!(media_kind(Path::new("icon.bmp")), EntryKind::Image);
        assert!(!reads_picture_size("svg"), "SVG has no size in pixels");
        assert!(reads_picture_size("tif"));
        assert_eq!(media_kind(Path::new("scene.glb")), EntryKind::File);
        assert_eq!(
            read_text_file(&write(
                "big.txt",
                &vec![b'a'; TEXT_PREVIEW_MAX_BYTES as usize + 1]
            )),
            None
        );
        assert!(read_text_file(&write(
            "full.txt",
            &vec![b'a'; TEXT_PREVIEW_MAX_BYTES as usize]
        ))
        .is_some());
        assert_eq!(read_text_file(root.path()), None);
        // A PDF or a picture is never text, however plain its bytes.
        assert_eq!(
            read_file(&write("Brief.pdf", b"%PDF-1.4\nBT (A page) Tj ET\n%%EOF\n")),
            FileRead::Pdf
        );
        assert_eq!(read_text_file(&root.path().join("Brief.pdf")), None);
        assert_eq!(
            read_file(&write("vector.SVG", b"<svg/>")),
            FileRead::Media(Media::Image)
        );
        assert_eq!(
            read_file(&write("scene.glb", b"glTF")),
            FileRead::Media(Media::Model)
        );
        assert_eq!(read_file(&write("book.xlsx", b"text")), FileRead::Workbook);
        assert_eq!(read_file(&write("book.ods", b"text")), FileRead::Workbook);
        assert_eq!(read_file(&write("Pitch.PPTX", b"text")), FileRead::Deck);
        // A system-preview file uses the previewer however plain its bytes, else opens
        // externally. Its text is never read as text.
        let previewed = if cfg!(target_os = "macos") {
            FileRead::SystemPreview
        } else {
            FileRead::External
        };
        for name in [
            "Pitch.ppt",
            "Pitch.key",
            "Letter.doc",
            "Brand.otf",
            "Cup.usdz",
            "Bag.obj",
        ] {
            assert_eq!(read_file(&write(name, b"text")), previewed, "{name}");
        }
        assert_eq!(read_text_file(&root.path().join("Bag.obj")), None);
        assert_eq!(
            read_file(&write("Card.html", b"<h1>A</h1>")),
            FileRead::Html("<h1>A</h1>".to_owned())
        );
        assert_eq!(
            read_text_file(&root.path().join("Card.html")).as_deref(),
            Some("<h1>A</h1>"),
            "opened as its source"
        );
        assert_eq!(
            read_file(&write("types.ts", b"let a = 1;")),
            FileRead::Text("let a = 1;".to_owned())
        );
        // An iPhone photo is a picture where the platform decodes it, else external.
        assert_eq!(
            read_file(&write("photo.heic", b"ftyp\0")),
            if phone_still_decodes_here("heic") {
                FileRead::Media(Media::Image)
            } else {
                FileRead::External
            }
        );
        assert_eq!(
            read_file(&write("scenes.html", b"<p>")),
            FileRead::Html("<p>".to_owned())
        );
        assert!(is_pdf_file(Path::new("/drive/Lease.PDF")) && is_pdf_file(Path::new("report.pdf")));
        assert!(!is_pdf_file(Path::new("/drive/pdf")) && !is_pdf_file(Path::new("notes.pdf.txt")));
    }

    #[test]
    fn renaming_keeps_the_kind_and_the_id_and_refuses_a_taken_name() {
        let (root, _metadata, mut workspace) = drive();
        let page = id_of(&mut workspace, &Location::ROOT, "AGENTS");
        workspace
            .rename(&Location::ROOT, &page, "Rules")
            .expect("rename");
        assert!(root.path().join("Rules.md").exists());
        assert_eq!(id_of(&mut workspace, &Location::ROOT, "Rules"), page);

        let text = id_of(&mut workspace, &Location::ROOT, "notes.txt");
        assert!(workspace
            .rename(&Location::ROOT, &text, "clip.MP4")
            .is_err());
        assert!(root.path().join("notes.txt").exists());
    }

    #[test]
    fn moving_a_folder_keeps_the_ids_of_what_it_holds() {
        let (root, _metadata, mut workspace) = drive();
        let folder = id_of(&mut workspace, &Location::ROOT, "Northroom");
        let folder_location = Location {
            folder_id: Some(folder.clone()),
        };
        let inner = id_of(&mut workspace, &folder_location, "Inner");
        let destination = workspace
            .create_folder(&Location::ROOT, "Archive")
            .expect("create");
        let destination_location = Location {
            folder_id: Some(destination),
        };

        workspace
            .move_items(
                &Location::ROOT,
                std::slice::from_ref(&folder),
                &destination_location,
            )
            .expect("move");
        assert!(root
            .path()
            .join("Archive")
            .join("Northroom")
            .join("Inner")
            .is_dir());
        assert_eq!(
            id_of(&mut workspace, &destination_location, "Northroom"),
            folder
        );
        assert_eq!(id_of(&mut workspace, &folder_location, "Inner"), inner);

        // A folder cannot move inside itself, and a file must be where the caller says it is.
        let inner_location = Location {
            folder_id: Some(inner),
        };
        assert!(workspace
            .move_items(
                &destination_location,
                std::slice::from_ref(&folder),
                &inner_location
            )
            .is_err());
        assert!(workspace
            .move_items(&Location::ROOT, &[folder], &destination_location)
            .is_err());
    }

    #[test]
    fn writing_a_text_file_keeps_its_id() {
        let (root, _metadata, mut workspace) = drive();
        let page = id_of(&mut workspace, &Location::ROOT, "AGENTS");
        workspace.write_text(&page, "# New words\n").expect("write");
        assert_eq!(
            fs::read_to_string(root.path().join("AGENTS.md")).expect("read"),
            "# New words\n"
        );
        assert_eq!(id_of(&mut workspace, &Location::ROOT, "AGENTS"), page);
        assert!(workspace.path_of(&page).is_ok());
    }

    #[test]
    fn deleting_goes_through_the_trash_and_forgets_the_file() {
        let (root, _metadata, mut workspace) = drive();
        let text = id_of(&mut workspace, &Location::ROOT, "notes.txt");
        assert!(
            workspace.delete(&Location::ROOT, &text).is_err(),
            "no trash, no delete"
        );
        assert!(root.path().join("notes.txt").exists());

        let bin = tempfile::tempdir().expect("bin");
        workspace.set_trash(trash_into(bin.path().to_path_buf()));
        let trashed = workspace.delete(&Location::ROOT, &text).expect("delete");
        assert!(!root.path().join("notes.txt").exists());
        assert_eq!(trashed.landed, Some(bin.path().join("notes.txt")));
        assert!(workspace.path_of(&text).is_err());
    }

    #[test]
    fn a_file_put_back_from_the_trash_keeps_its_id_and_those_of_what_it_holds() {
        let (_root, _metadata, mut workspace) = drive();
        let bin = tempfile::tempdir().expect("bin");
        workspace.set_trash(trash_into(bin.path().to_path_buf()));
        let folder = id_of(&mut workspace, &Location::ROOT, "Northroom");
        let inner = id_of(
            &mut workspace,
            &Location {
                folder_id: Some(folder.clone()),
            },
            "Inner",
        );
        let path = workspace.root().join("Northroom");
        let trashed = workspace.trash_path(&path).expect("trash");
        let landed = trashed.landed.clone().expect("the check's trash says");
        assert!(workspace.path_of(&inner).is_err());
        workspace
            .restore(&landed, &trashed.kept, &path)
            .expect("restore");
        assert_eq!(workspace.path_of(&folder).expect("folder"), path);
        assert_eq!(
            workspace.path_of(&inner).expect("inner"),
            path.join("Inner")
        );
        let entries = workspace.all_contents(&Location::ROOT).expect("contents");
        assert!(entries.iter().any(|entry| entry.id == folder));
        // A destination taken since is refused, and nothing moves.
        let again = workspace.trash_path(&path).expect("trash again");
        fs::create_dir(&path).expect("taken");
        assert!(workspace
            .restore(again.landed.as_deref().expect("landed"), &again.kept, &path)
            .is_err());
        assert!(again.landed.expect("landed").exists());
    }

    /// A folder outside the drive holding `Album/`: a file, a folder with a file, and a link.
    fn album() -> tempfile::TempDir {
        let outside = tempfile::tempdir().expect("outside");
        let album = outside.path().join("Album");
        fs::create_dir_all(album.join("Inner")).expect("album");
        fs::write(album.join("a.txt"), b"a").expect("a");
        fs::write(album.join("Inner").join("b.txt"), b"b").expect("b");
        #[cfg(unix)]
        std::os::unix::fs::symlink("a.txt", album.join("link")).expect("link");
        outside
    }

    #[test]
    fn copies_files_and_whole_folders_in_numbered_never_replacing() {
        let (root, _metadata, mut workspace) = drive();
        let outside = album();
        let album_path = outside.path().join("Album");
        fs::write(outside.path().join("notes.txt"), b"new").expect("notes");
        let ids = workspace
            .copy_in(
                &Location::ROOT,
                &[album_path.clone(), outside.path().join("notes.txt")],
            )
            .expect("copy");
        assert_eq!(ids.len(), 2);
        let copied = workspace.root().join("Album");
        assert_eq!(workspace.path_of(&ids[0]).expect("album"), copied);
        assert_eq!(fs::read(copied.join("a.txt")).expect("a"), b"a");
        assert_eq!(fs::read(copied.join("Inner/b.txt")).expect("b"), b"b");
        #[cfg(unix)]
        assert_eq!(
            fs::read_link(copied.join("link")).expect("a link stays a link"),
            Path::new("a.txt")
        );
        // "notes.txt" was taken, so the copy is numbered and the original kept.
        assert_eq!(fs::read(root.path().join("notes.txt")).expect("old"), b"x");
        assert_eq!(
            fs::read(root.path().join("notes 2.txt")).expect("numbered"),
            b"new"
        );
        assert_eq!(
            workspace.path_of(&ids[1]).expect("notes"),
            workspace.root().join("notes 2.txt")
        );
        // A folder copied again is numbered too, before any extension.
        workspace
            .copy_in(
                &Location::ROOT,
                &[album_path, root.path().join("Cut.timeline")],
            )
            .expect("again");
        assert!(root.path().join("Album 2/Inner/b.txt").exists());
        assert!(root.path().join("Cut 2.timeline").is_dir());
    }

    #[test]
    fn copies_from_inside_the_drive_and_never_a_folder_into_itself() {
        let (root, _metadata, mut workspace) = drive();
        let folder = id_of(&mut workspace, &Location::ROOT, "Northroom");
        let northroom = Location {
            folder_id: Some(folder.clone()),
        };
        let inner = Location {
            folder_id: Some(id_of(&mut workspace, &northroom, "Inner")),
        };
        workspace
            .copy_in(&northroom, &[root.path().join("notes.txt")])
            .expect("into a folder");
        assert!(root.path().join("Northroom/notes.txt").exists());
        assert!(root.path().join("notes.txt").exists(), "a copy, not a move");

        let source = root.path().join("Northroom");
        for (into, what) in [(&northroom, "itself"), (&inner, "a folder inside it")] {
            let refused = workspace
                .copy_in(into, &[root.path().join("notes.txt"), source.clone()])
                .expect_err(what);
            assert_eq!(
                refused.to_string(),
                "“Northroom” can’t be copied into itself"
            );
        }
        assert!(
            !root.path().join("Northroom/notes 2.txt").exists(),
            "nothing is copied before every source is checked"
        );
        // Beside itself, as Duplicate does, it is numbered.
        workspace
            .copy_in(&Location::ROOT, &[source])
            .expect("beside itself");
        assert!(root.path().join("Northroom 2/Inner").is_dir());
    }

    #[test]
    fn refuses_a_file_as_a_folder_and_lists_long_folders_whole() {
        let (root, _metadata, mut workspace) = drive();
        let top = workspace.all_contents(&Location::ROOT).expect("contents");
        let page = top
            .iter()
            .find(|entry| entry.kind == EntryKind::Page)
            .expect("page");
        assert!(workspace
            .all_contents(&Location {
                folder_id: Some(page.id.clone())
            })
            .is_err());

        let many = root.path().join("Northroom").join("Inner");
        let count = 405;
        for index in 0..count {
            fs::write(many.join(format!("file-{index:04}.txt")), b"x").expect("file");
        }
        let folder = top
            .iter()
            .find(|entry| entry.title == "Northroom")
            .expect("folder");
        let inner = workspace
            .all_contents(&Location {
                folder_id: Some(folder.id.clone()),
            })
            .expect("inner");
        let inner_location = Location {
            folder_id: Some(inner[0].id.clone()),
        };
        let listed = workspace.all_contents(&inner_location).expect("all");
        assert_eq!(listed.len(), count);
        assert!(listed
            .windows(2)
            .all(|pair| compare_names(&pair[0].title, &pair[1].title).is_lt()));
    }
}
