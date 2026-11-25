//! File list text and ordering: Finder-style sizes and dates, the Kind column, and column
//! sorting. `tests/list_parity.rs` checks it against recorded sizes, dates and names.

pub mod header;
pub mod moves;
pub mod row_keys;
pub mod row_pointer;

use crate::workspace::{Entry, EntryKind, FileFamily, Indexed};
use std::cmp::Ordering;

const BYTE_UNITS: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Formats a size as Finder does: decimal units, one decimal place from MB up, "--" if unknown.
pub fn format_size(size_bytes: Option<u64>) -> String {
    let Some(bytes) = size_bytes else {
        return "--".to_owned();
    };
    if bytes == 0 {
        return "Zero bytes".to_owned();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < BYTE_UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    if unit <= 1 {
        // Halves round up.
        format!("{} {}", (value + 0.5).floor(), BYTE_UNITS[unit])
    } else {
        // A half rounds up; Rust's formatting rounds it to even.
        let tenths = (value * 10.0 + 0.5).floor() as u64;
        format!("{}.{} {}", tenths / 10, tenths % 10, BYTE_UNITS[unit])
    }
}

/// A moment in the local time zone and calendar.
pub(crate) struct Local {
    pub(crate) year: i32,
    /// 0 for January.
    pub(crate) month: usize,
    pub(crate) day: i32,
    pub(crate) hour: i32,
    pub(crate) minute: i32,
    pub(crate) second: i32,
    /// Days since a fixed epoch, for counting whole days between two moments.
    pub(crate) day_number: i64,
}

pub(crate) fn local(seconds: i64) -> Option<Local> {
    let mut parts: libc::tm = unsafe { std::mem::zeroed() };
    let time: libc::time_t = seconds;
    // SAFETY: both pointers are to live values of the right types.
    #[cfg(unix)]
    let read = !unsafe { libc::localtime_r(&time, &mut parts) }.is_null();
    // SAFETY: as above.
    #[cfg(windows)]
    let read = unsafe { libc::localtime_s(&mut parts, &time) } == 0;
    if !read {
        return None;
    }
    // Count civil days so a day with a DST change still counts as one.
    let day_number = crate::calendar::days_from_civil(
        i64::from(parts.tm_year) + 1900,
        i64::from(parts.tm_mon) + 1,
        i64::from(parts.tm_mday),
    );
    Some(Local {
        year: parts.tm_year + 1900,
        month: parts.tm_mon as usize,
        day: parts.tm_mday,
        hour: parts.tm_hour,
        minute: parts.tm_min,
        second: parts.tm_sec,
        day_number,
    })
}

/// Formats a date as Finder does: "Today at 3:58 PM", "Yesterday at …", else
/// "Oct 10, 2026 at 3:58 PM".
pub fn format_date(seconds: Option<i64>, now_seconds: i64) -> String {
    let (Some(time), Some(now)) = (seconds.and_then(local), local(now_seconds)) else {
        return "--".to_owned();
    };
    let clock = clock_of(time.hour as u32, time.minute as u32);
    match now.day_number - time.day_number {
        0 => format!("Today at {clock}"),
        1 => format!("Yesterday at {clock}"),
        _ => dated(time.month, time.day as u32, time.year as i64, &clock),
    }
}

/// A time of day as Finder says it: "3:58 PM".
pub fn clock_of(hour: u32, minute: u32) -> String {
    let twelve = match hour % 12 {
        0 => 12,
        other => other,
    };
    format!(
        "{twelve}:{minute:02} {}",
        if hour < 12 { "AM" } else { "PM" }
    )
}

/// A day and time as Finder says it: "Oct 10, 2026 at 3:58 PM". `month` counts from 0.
pub fn dated(month: usize, day: u32, year: i64, clock: &str) -> String {
    format!("{} {day}, {year} at {clock}", MONTHS[month.min(11)])
}

/// The Kind column text, as Finder would word it.
pub fn describe_kind(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::Folder => "Folder",
        EntryKind::Page => "Page",
        EntryKind::Terminal => "Terminal",
        EntryKind::Image => "Image",
        EntryKind::Video => "Video",
        EntryKind::Audio => "Audio",
        EntryKind::File => "File",
    }
}

/// The text after the last dot of a name, or the whole name if it has none (`Makefile`; `.zshrc`
/// gives `zshrc`). Picks the syntax highlighting grammar.
pub fn name_extension(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// The header text after "Name": the selected count, or the listed count when none is selected.
pub fn count_label(listed: usize, selected: usize) -> String {
    match (selected, listed) {
        (0, 1) => "1 item".to_owned(),
        (0, _) => format!("{listed} items"),
        _ => format!("{selected} selected"),
    }
}

/// The label on the drag chip: the file name for one file, or "3 items" for several.
pub fn drag_label(count: usize, first_title: &str) -> String {
    let only = first_title.trim();
    match count {
        0 | 1 if only.is_empty() => "1 item".to_owned(),
        0 | 1 => only.to_owned(),
        _ => format!("{count} items"),
    }
}

/// Horizontal positions of a row's parts, in points from the left of the Name column (list) or
/// the row (columns): the 16 pt caret slot, the 24 pt icon slot, the name, the rename field, and
/// the placeholder text for an open folder's contents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowLayout {
    pub caret: f64,
    pub icon: f64,
    pub name: f64,
    pub rename: f64,
    pub placeholder: f64,
    /// Row height, which is also the row pitch and the hit height.
    pub height: f64,
}

/// Row height in the list and in columns.
const LIST_ROW: f64 = 32.0;
const COLUMN_ROW: f64 = 28.0;

/// Indent per level of folder nesting in the list.
const DEPTH_INDENT: f64 = 16.0;

/// A list row `depth` folders deep: caret slot, 6 pt, icon slot, 6 pt, then the name. The rename
/// field starts 4 pt before the name so its text lines up with it.
pub fn list_row_layout(depth: u32) -> RowLayout {
    let indent = f64::from(depth) * DEPTH_INDENT;
    RowLayout {
        caret: indent,
        icon: indent + 22.0,
        name: indent + 52.0,
        rename: indent + 48.0,
        placeholder: indent + 30.0,
        height: LIST_ROW,
    }
}

/// A row in column view: icon 10 pt in, then the name, with a folder's caret at the end.
pub fn column_row_layout() -> RowLayout {
    RowLayout {
        caret: 217.0,
        icon: 10.0,
        name: 40.0,
        rename: 36.0,
        placeholder: 40.0,
        height: COLUMN_ROW,
    }
}

pub use crate::glyph::Glyph;

/// The glyph for a file of `kind` when no picture is drawn: in a row's icon slot, under a picture
/// still loading, and in the details pane. Pages use the generic sheet; source code and archives
/// have their own.
pub fn file_glyph(kind: EntryKind, family: FileFamily) -> Glyph {
    match kind {
        EntryKind::Folder => Glyph::Folder,
        EntryKind::Terminal => Glyph::TerminalWindow,
        EntryKind::Image => Glyph::Image,
        EntryKind::Video => Glyph::FilmStrip,
        EntryKind::Audio => Glyph::MusicNote,
        EntryKind::Page => Glyph::File,
        EntryKind::File => match family {
            FileFamily::Code => Glyph::FileCode,
            FileFamily::Archive => Glyph::FileZip,
            FileFamily::Other => Glyph::File,
        },
    }
}

/// Sort rank of a character in dictionary order: spaces, punctuation, digits, then letters.
fn rank(character: char) -> (u8, u32) {
    const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    if character.is_whitespace() {
        (0, 0)
    } else if let Some(position) = PUNCTUATION.find(character) {
        (1, position as u32)
    } else if character.is_ascii_digit() {
        (2, character as u32)
    } else if character.is_alphanumeric() {
        (
            3,
            character.to_lowercase().next().unwrap_or(character) as u32,
        )
    } else {
        (1, 1000 + character as u32)
    }
}

/// Compares names in dictionary order, with numbers by value and case ignored: "file2" before
/// "file10", "Beta" beside "beta". Names that differ only in case or leading zeros compare equal,
/// so a stable sort keeps their order.
pub fn compare_names(left: &str, right: &str) -> Ordering {
    let (left, right): (Vec<char>, Vec<char>) = (left.chars().collect(), right.chars().collect());
    let (mut a, mut b) = (0, 0);
    while a < left.len() && b < right.len() {
        if left[a].is_ascii_digit() && right[b].is_ascii_digit() {
            let run = |text: &[char], from: usize| {
                let end = (from..text.len())
                    .find(|&index| !text[index].is_ascii_digit())
                    .unwrap_or(text.len());
                let digits: String = text[from..end].iter().collect();
                (digits.trim_start_matches('0').to_owned(), end)
            };
            let ((left_digits, left_end), (right_digits, right_end)) =
                (run(&left, a), run(&right, b));
            let by_value = left_digits
                .len()
                .cmp(&right_digits.len())
                .then_with(|| left_digits.cmp(&right_digits));
            if by_value != Ordering::Equal {
                return by_value;
            }
            (a, b) = (left_end, right_end);
            continue;
        }
        let by_character = rank(left[a]).cmp(&rank(right[b]));
        if by_character != Ordering::Equal {
            return by_character;
        }
        a += 1;
        b += 1;
    }
    (left.len() - a).cmp(&(right.len() - b))
}

/// The list columns, in the chooser's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Name,
    Modified,
    Created,
    Size,
    Kind,
    Added,
    Opened,
    Accessed,
    AttributesModified,
    Extension,
    Location,
    Owner,
    Permissions,
    Dimensions,
    Duration,
    Camera,
    Author,
    Comment,
    DownloadSource,
}

/// The column a list is sorted by.
pub type SortKey = Column;

impl Column {
    pub const ALL: [Column; 19] = [
        Column::Name,
        Column::Modified,
        Column::Created,
        Column::Size,
        Column::Kind,
        Column::Added,
        Column::Opened,
        Column::Accessed,
        Column::AttributesModified,
        Column::Extension,
        Column::Location,
        Column::Owner,
        Column::Permissions,
        Column::Dimensions,
        Column::Duration,
        Column::Camera,
        Column::Author,
        Column::Comment,
        Column::DownloadSource,
    ];
    /// The default visible columns.
    pub const DEFAULT: [Column; 4] = [Column::Name, Column::Modified, Column::Size, Column::Kind];

    pub fn number(self) -> u32 {
        Self::ALL
            .iter()
            .position(|column| *column == self)
            .unwrap_or(0) as u32
    }

    pub fn numbered(number: u32) -> Option<Self> {
        Self::ALL.get(number as usize).copied()
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Modified => "Date Modified",
            Self::Created => "Date Created",
            Self::Size => "Size",
            Self::Kind => "Kind",
            Self::Added => "Date Added",
            Self::Opened => "Date Last Opened",
            Self::Accessed => "Date Last Accessed",
            Self::AttributesModified => "Attributes Modified",
            Self::Extension => "Extension",
            Self::Location => "Location",
            Self::Owner => "Owner",
            Self::Permissions => "Permissions",
            Self::Dimensions => "Dimensions",
            Self::Duration => "Duration",
            Self::Camera => "Camera",
            Self::Author => "Author",
            Self::Comment => "Comments",
            Self::DownloadSource => "Download Source",
        }
    }

    /// The column's default, minimum and maximum width. Name has none; it takes the remaining
    /// room.
    pub fn widths(self) -> (f64, f64, f64) {
        match self {
            Self::Name => (0.0, 0.0, 0.0),
            Self::Modified
            | Self::Created
            | Self::Added
            | Self::Opened
            | Self::Accessed
            | Self::AttributesModified => (176.0, 96.0, 360.0),
            Self::Size => (80.0, 48.0, 200.0),
            Self::Kind => (112.0, 64.0, 280.0),
            Self::Extension => (96.0, 64.0, 200.0),
            Self::Location => (260.0, 120.0, 800.0),
            Self::Owner => (120.0, 64.0, 280.0),
            Self::Permissions => (100.0, 64.0, 240.0),
            Self::Dimensions => (140.0, 80.0, 280.0),
            Self::Duration => (100.0, 64.0, 240.0),
            Self::Camera => (180.0, 80.0, 480.0),
            Self::Author => (160.0, 80.0, 480.0),
            Self::Comment => (240.0, 120.0, 800.0),
            Self::DownloadSource => (260.0, 120.0, 800.0),
        }
    }

    /// Clamps a dragged width to the column's limits and rounds it.
    pub fn clamp_width(self, width: f64) -> f64 {
        let (_, least, most) = self.widths();
        width.clamp(least, most).round()
    }
}

/// A file's value in one column.
enum Value {
    Text(String),
    Date(i64),
    Size(u64),
    /// Width and height in pixels; ordered by their product.
    Dimensions(u32, u32),
    /// Seconds.
    Duration(f64),
    Unknown,
}

impl Column {
    /// Whether the column's values come from the system's file index.
    pub fn needs_index(self) -> bool {
        matches!(
            self,
            Self::Added
                | Self::Opened
                | Self::Dimensions
                | Self::Duration
                | Self::Camera
                | Self::Author
                | Self::Comment
                | Self::DownloadSource
        )
    }
}

/// Formats seconds to a tenth, dropping `.0`: "12 s", "3.5 s".
pub fn format_duration(seconds: f64) -> String {
    let tenths = (seconds * 10.0 + 0.5).floor() / 10.0;
    if tenths.fract() == 0.0 {
        format!("{tenths:.0} s")
    } else {
        format!("{tenths:.1} s")
    }
}

/// A file's value in `column`. Index-only values (added, opened, dimensions, duration, camera,
/// author, comment, download source) are present once the listing has requested them.
fn value(entry: &Entry, column: Column) -> Value {
    let text =
        |words: Option<&str>| words.map_or(Value::Unknown, |words| Value::Text(words.to_owned()));
    let date = |seconds: Option<i64>| seconds.map_or(Value::Unknown, Value::Date);
    match column {
        Column::Name => Value::Text(entry.title.clone()),
        Column::Modified => Value::Date(entry.modified_seconds),
        Column::Created => date(entry.created_seconds),
        Column::Size => entry.size_bytes.map_or(Value::Unknown, Value::Size),
        // The platform's own name for the file's kind.
        Column::Kind => Value::Text(
            entry
                .kind_description
                .clone()
                .unwrap_or_else(|| describe_kind(entry.kind).to_owned()),
        ),
        Column::Accessed => date(entry.accessed_seconds),
        Column::AttributesModified => date(entry.changed_seconds),
        Column::Extension => text(entry.extension.as_deref()),
        Column::Location => text(Some(&entry.location)),
        Column::Owner => text(Some(&entry.owner)),
        Column::Permissions => text(Some(&entry.permissions)),
        Column::Added
        | Column::Opened
        | Column::Dimensions
        | Column::Duration
        | Column::Camera
        | Column::Author
        | Column::Comment
        | Column::DownloadSource => entry
            .indexed
            .as_ref()
            .map_or(Value::Unknown, |indexed| indexed_value(indexed, column)),
    }
}

/// A file's value in an index-only column.
fn indexed_value(indexed: &Indexed, column: Column) -> Value {
    let text =
        |words: Option<&str>| words.map_or(Value::Unknown, |words| Value::Text(words.to_owned()));
    let date = |seconds: Option<i64>| seconds.map_or(Value::Unknown, Value::Date);
    let joined = |words: &[String]| {
        if words.is_empty() {
            Value::Unknown
        } else {
            Value::Text(words.join(", "))
        }
    };
    match column {
        Column::Added => date(indexed.added_seconds),
        Column::Opened => date(indexed.last_opened_seconds),
        Column::Dimensions => match (indexed.pixel_width, indexed.pixel_height) {
            (Some(width), Some(height)) => Value::Dimensions(width, height),
            _ => Value::Unknown,
        },
        Column::Duration => indexed
            .duration_seconds
            .map_or(Value::Unknown, Value::Duration),
        Column::Camera => text(indexed.camera.as_deref()),
        Column::Author => joined(&indexed.authors),
        Column::Comment => text(indexed.comment.as_deref()),
        Column::DownloadSource => joined(&indexed.download_sources),
        // Read from the file itself, not the index.
        Column::Name
        | Column::Modified
        | Column::Created
        | Column::Size
        | Column::Kind
        | Column::Accessed
        | Column::AttributesModified
        | Column::Extension
        | Column::Location
        | Column::Owner
        | Column::Permissions => Value::Unknown,
    }
}

/// The cell text for one file in `column`; "--" when unknown.
pub fn cell(entry: &Entry, column: Column, now_seconds: i64) -> String {
    match value(entry, column) {
        Value::Text(words) => words,
        Value::Date(seconds) => format_date(Some(seconds), now_seconds),
        Value::Size(bytes) => format_size(Some(bytes)),
        Value::Dimensions(width, height) => format!("{width} × {height}"),
        Value::Duration(seconds) => format_duration(seconds),
        Value::Unknown => "--".to_owned(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub ascending: bool,
}

/// A header click: a new column sorts ascending, the same column flips direction.
pub fn toggle_sort(current: Option<Sort>, key: SortKey) -> Sort {
    match current {
        Some(sort) if sort.key == key => Sort {
            key,
            ascending: !sort.ascending,
        },
        _ => Sort {
            key,
            ascending: true,
        },
    }
}

fn compare_by(left: &Entry, right: &Entry, key: SortKey) -> Ordering {
    match (value(left, key), value(right, key)) {
        (Value::Text(a), Value::Text(b)) => compare_names(&a, &b),
        (Value::Date(a), Value::Date(b)) => a.cmp(&b),
        (Value::Size(a), Value::Size(b)) => a.cmp(&b),
        (Value::Dimensions(aw, ah), Value::Dimensions(bw, bh)) => {
            (u64::from(aw) * u64::from(ah)).cmp(&(u64::from(bw) * u64::from(bh)))
        }
        (Value::Duration(a), Value::Duration(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    }
}

/// Sorts siblings by the chosen column (ties by name), or keeps the folder's own order. Unknown
/// values sort last in either direction.
pub fn sort_entries(entries: &mut [Entry], sort: Option<Sort>, folders_first: bool) {
    if sort.is_none() && !folders_first {
        return;
    }
    entries.sort_by(|left, right| {
        let folders = if folders_first {
            folders_before_files(left, right)
        } else {
            Ordering::Equal
        };
        folders.then_with(|| sort.map_or(Ordering::Equal, |sort| compare_sorted(left, right, sort)))
    });
}

fn folders_before_files(left: &Entry, right: &Entry) -> Ordering {
    let is_folder = |entry: &Entry| entry.kind == EntryKind::Folder;
    is_folder(right).cmp(&is_folder(left))
}

/// Compares two files by `sort`'s column, ties by name. Unknown values sort last in either
/// direction.
fn compare_sorted(left: &Entry, right: &Entry, sort: Sort) -> Ordering {
    let unknown = |entry: &Entry| matches!(value(entry, sort.key), Value::Unknown);
    unknown(left)
        .cmp(&unknown(right))
        .then_with(|| {
            let ordered = compare_by(left, right, sort.key);
            if sort.ascending {
                ordered
            } else {
                ordered.reverse()
            }
        })
        .then_with(|| compare_by(left, right, Column::Name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_is_coloured_by_what_its_name_ends_in() {
        assert_eq!(name_extension("main.rs"), "rs");
        assert_eq!(name_extension("archive.tar.gz"), "gz");
        assert_eq!(name_extension("Makefile"), "Makefile");
        assert_eq!(name_extension(".zshrc"), "zshrc");
        assert_eq!(name_extension("notes."), "");
    }

    #[test]
    fn the_header_counts_the_files_or_the_selection() {
        assert_eq!(count_label(1, 0), "1 item");
        assert_eq!(count_label(0, 0), "0 items");
        assert_eq!(count_label(12, 0), "12 items");
        assert_eq!(count_label(12, 3), "3 selected");
    }

    #[test]
    fn a_drag_chip_names_one_file_or_counts_several() {
        assert_eq!(drag_label(1, " cut.mp4 "), "cut.mp4");
        assert_eq!(drag_label(1, "  "), "1 item");
        assert_eq!(drag_label(3, "cut.mp4"), "3 items");
    }

    #[test]
    fn a_list_row_moves_in_16_for_each_open_folder_it_is_inside() {
        assert_eq!(
            list_row_layout(0),
            RowLayout {
                caret: 0.0,
                icon: 22.0,
                name: 52.0,
                rename: 48.0,
                placeholder: 30.0,
                height: 32.0
            }
        );
        assert_eq!(list_row_layout(2).caret, 32.0);
        assert_eq!(
            (list_row_layout(0).height, column_row_layout().height),
            (32.0, 28.0)
        );
        assert_eq!(list_row_layout(2).name, 84.0);
        assert_eq!(column_row_layout().rename, 36.0);
    }

    #[test]
    fn a_file_without_a_picture_shows_its_kinds_glyph() {
        let other = FileFamily::Other;
        assert_eq!(file_glyph(EntryKind::Folder, other), Glyph::Folder);
        assert_eq!(file_glyph(EntryKind::Page, other), Glyph::File);
        assert_eq!(
            file_glyph(EntryKind::File, FileFamily::Code),
            Glyph::FileCode
        );
        assert_eq!(
            file_glyph(EntryKind::File, FileFamily::Archive),
            Glyph::FileZip
        );
        assert_eq!(file_glyph(EntryKind::File, other), Glyph::File);
        assert_eq!(file_glyph(EntryKind::Video, other), Glyph::FilmStrip);
        assert_eq!(
            file_glyph(EntryKind::Terminal, other).name(),
            "terminalWindow"
        );
        assert_eq!(FileFamily::of_extension("zip"), FileFamily::Archive);
        assert_eq!(FileFamily::of_extension("rs"), FileFamily::Code);
        assert_eq!(FileFamily::of_extension("pdf"), FileFamily::Other);
        assert_eq!(
            FileFamily::of_path(std::path::Path::new("Old/Archive.ZIP")),
            FileFamily::Archive
        );
    }
}
