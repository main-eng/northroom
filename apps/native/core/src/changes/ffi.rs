//! The Changes tab's boundary (ADR-0001): what changed in a root (`changes`), listed and diffed
//! by the core so the shell only draws. A Changes tab is a tab of the title bar's strip
//! (`Columns::open_changes_tab`); everything here reads the root of the active one. Its picker,
//! on the list's first line, chooses the scope (`view::Scope`): New (what agents changed since
//! the person last kept it), Uncommitted and vs main (what git says differs). It shows a flat
//! list of the files and one continuous diff of them all, a section per file; the file whose
//! section holds the diff's top edge is in view, lit in the list, and its name line stays pinned
//! at the diff's top. Under the pointer a name line ends in Keep and Undo for its file, and a
//! hunk in a strip of the same two at its first row's right end; vs main is read only and has
//! neither, Uncommitted has only Undo. Undo asks nothing: what it replaces goes to the Trash, and
//! ⌘Z right after puts it back. A section collapses to its name line when the person folds it, or
//! when it starts so as noise (`view::starts_folded`). Text is borrowed UTF-8, valid until the
//! next `nr_changes_load` (or the next call that returned it).
//!
//! The surface, in the order a screen uses it:
//!
//! - `nr_changes_open(folder: *const c_char)`: opens or selects the Changes tab of an absolute
//!   folder path's root (its repository, else the drive; `git::root_for`).
//! - `nr_changes_open_current()`: the same for the root with the latest turn, else the folder
//!   on screen (the menu bar's Show Changes, the title bar's review button).
//! - `nr_changes_waiting() -> usize`: how many files wait for review; the title bar shows its
//!   review button while any do.
//! - `nr_changes_shown() -> bool`: whether the active tab is a Changes tab
//!   (`NrShellView.changes`).
//! - `nr_changes_revision() -> u64`: changes whenever anything below may read differently.
//! - `nr_changes_words() -> NrChangesWords`: every fixed word the screen draws.
//! - `nr_changes_load() -> NrChangesLoaded`: lists every file of the scope shown, with its
//!   diff, and says which is in view and the offset that keeps it so.
//! - `nr_changes_picker() -> NrChangesPicker`: the picker's words ("New · 4") and what an empty
//!   scope says; `menu_changes_scope()` is its menu (`NrMenu::ChangesScope`).
//! - `nr_changes_file(index, *mut NrChangesFile) -> bool`: one file: its row and its section.
//! - `nr_changes_texts(index) -> NrChangesTexts`: its extension and both sides' texts for
//!   `nr_syntax_call`.
//! - `nr_changes_diff_row(file, index, *mut NrChangesRow) -> bool`: one drawn row of its diff.
//! - `nr_changes_word(file, index) -> NrChangesRange`: a changed word in a row's text.
//! - `nr_changes_scrolled(offset) -> i64`: the scroll-spy; the file now in view.
//! - `nr_changes_select(index) -> NrChangesKey`: a click in the list; it scrolls there and
//!   expands the file if collapsed.
//! - `nr_changes_frames(width, height) -> NrChangesFrames`: where the parts go.
//! - `nr_changes_pinned(offset) -> NrChangesPinned`: the name line pinned at the diff's top.
//! - `nr_changes_line_tools(index, top, hovered) -> NrChangesLineTools`: a name line's tools.
//! - `nr_changes_strip(hunk, offset) -> NrChangesStrip`: a hunk's tools, and
//!   `nr_changes_current_hunk() -> i64`, the hunk N and P stepped to, which shows its strip too.
//! - `nr_changes_hit(offset, x, y) -> NrChangesHit`: what a point is on: a name line's tool, a
//!   name line, which a click folds (`nr_changes_fold`, ⌥ every file), a hunk's tool or a hunk.
//! - `nr_changes_key(NrKeyPress) -> NrChangesKey`: J/K or Down/Up files, N/P hunks, X folds the
//!   file in view (⌥X every file), ⌘1–⌘3 scopes, ⌘Y keeps the current hunk and ⌘⌥Z undoes it
//!   (⌘⇧Y and ⌘⇧⌥Z its whole file), ⌘Z puts the last Undo back.
//! - The tools: `nr_changes_file_tool(index, tool)` and `nr_changes_hunk_tool(hunk, tool)`, each
//!   `-> NrMenuEffect` (a redraw, or a notice of why it failed); Keep all and Undo all are in
//!   the picker's menu.
//!
//! Beside these: `NrShellView.changes` (`nr_shell_view`), the `changes` flag of
//! `nr_window_layout`, tab kind 6 with the `diff` glyph, and the menus' Show Changes (View
//! menu, a folder's menu, a folder's background menu) and the title bar's review button.

use crate::changes::actions::{self, ActionError, Undone};
use crate::changes::diff::{self, Cell, FileDiff, LineKind};
use crate::changes::frames::{self, Sections};
use crate::changes::git;
use crate::changes::service::Service;
use crate::changes::turns::{relative_to, Engine, Signal, TurnTap};
use crate::changes::view::{self, FileRow, Scope, Status, ViewState};
use crate::ffi::{interface_language, text_of, with_columns, NrKey, NrKeyPress, NrRect, NrText};
use crate::glyph::Glyph;
use crate::menus::ffi::NrMenuEffect;
use crate::menus::{self, Entry, Shortcut};
use crate::strings::{self, key};
use crate::Relock;
use std::ffi::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The changes' folder in the app's own.
const CHANGES_FOLDER: &str = "changes";

/// The changes of the running app.
pub(crate) fn service() -> &'static Service {
    crate::app().changes.get_or_init(|| {
        let folder = crate::paths::get().support.join(CHANGES_FOLDER);
        Service::start(
            &folder,
            Arc::new(crate::changes::SystemClock),
            crate::notify_changed,
        )
    })
}

/// Follows the session of terminal file `id` at `file`, whose shell runs in `folder` of the
/// drive `drive`, and returns what its terminal reports through. Its changes count in the
/// folder's root: its repository, else the drive (`git::root_for`), and are credited to the
/// file's path in the root, which stays the same across runs.
pub(crate) fn follow_terminal(
    id: &str,
    file: &Path,
    folder: &Path,
    drive: Option<&Path>,
) -> TurnTap {
    let root = drive.map_or_else(
        || folder.to_path_buf(),
        |drive| git::root_for(folder, drive),
    );
    let credit = relative_to(&root, file).unwrap_or_else(|| file.to_string_lossy().into_owned());
    service().add_terminal(id, &root, &credit);
    let id = id.to_owned();
    TurnTap::new(move |signal: Signal| service().signal(&id, signal))
}

/// Stops following terminal file `id`'s session, which ended.
pub(crate) fn unfollow_terminal(id: &str) {
    if crate::app().changes.get().is_some() {
        service().remove_terminal(id);
    }
}

/// Keeps what every root's Changes list needs across restarts now. Call when the app quits.
pub(crate) fn save_now() {
    if crate::app().changes.get().is_some() {
        service().save_now();
    }
}

// MARK: Opening

/// Opens or selects the Changes tab of `folder`'s root, an absolute path. Settings close.
///
/// # Safety
/// `folder` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_changes_open(folder: *const c_char) {
    // SAFETY: the caller passes a NUL-terminated string.
    let folder = PathBuf::from(unsafe { text_of(folder) });
    open_folder(&folder);
}

/// Opens or selects the Changes tab of `folder`'s root: its repository, else the drive.
pub(crate) fn open_folder(folder: &Path) {
    let root = root_of(folder);
    crate::ffi::nr_settings_close();
    let opened = with_columns(|columns| columns.open_changes_tab(&root));
    match opened {
        Some(Ok(())) => service().open_view(&root),
        Some(Err(error)) => eprintln!("the Changes tab did not open: {error}"),
        None => {}
    }
}

/// `folder`'s root: its repository, else the drive.
fn root_of(folder: &Path) -> PathBuf {
    let drive = with_columns(|columns| columns.workspace().root().to_path_buf());
    drive.map_or_else(
        || folder.to_path_buf(),
        |drive| git::root_for(folder, &drive),
    )
}

/// The folder whose Changes Show Changes and Review changes open: the root with the latest
/// turn, else the folder on screen.
fn current_folder() -> Option<PathBuf> {
    let busiest = service().read(|engine| engine.busiest_root().map(Path::to_path_buf));
    busiest.or_else(folder_on_screen)
}

/// Opens the Changes tab of the root with the latest turn, else of the folder on screen.
#[no_mangle]
pub extern "C" fn nr_changes_open_current() {
    if let Some(folder) = current_folder() {
        open_folder(&folder);
    }
}

/// How many files the list Review changes opens holds: the New list of the root with the latest
/// turn, else of the folder on screen.
fn waiting() -> usize {
    current_folder().map_or(0, |folder| service().waiting(&root_of(&folder)))
}

/// How many files wait for review (`waiting`). The title bar shows its review button while any
/// do.
#[no_mangle]
pub extern "C" fn nr_changes_waiting() -> usize {
    waiting()
}

/// The review button's tooltip: "Review changes · 4 files", "… · 1 file", plain at none.
pub(crate) fn review_title() -> String {
    review_words(waiting(), interface_language())
}

fn review_words(waiting: usize, language: &str) -> String {
    use crate::strings::{self, key};
    let count = waiting.to_string();
    match waiting {
        0 => strings::text(language, key::MENU_REVIEW_CHANGES).to_owned(),
        1 => strings::format(
            language,
            key::MENU_REVIEW_CHANGES_FILE,
            &[("count", &count)],
        ),
        _ => strings::format(
            language,
            key::MENU_REVIEW_CHANGES_FILES,
            &[("count", &count)],
        ),
    }
}

/// The folder the active tab's last column shows.
fn folder_on_screen() -> Option<PathBuf> {
    with_columns(|columns| {
        let location = columns.columns().last()?.location.clone();
        columns.workspace().require_directory(&location).ok()
    })
    .flatten()
}

/// The root the active tab shows the changes of, when it is a Changes tab.
fn shown_root() -> Option<PathBuf> {
    with_columns(|columns| columns.active_changes().map(Path::to_path_buf)).flatten()
}

/// Whether the active tab is a Changes tab.
#[no_mangle]
pub extern "C" fn nr_changes_shown() -> bool {
    shown_root().is_some()
}

/// A tab showing `root` closed: once no other tab shows it, it is no longer watched for it.
pub(crate) fn tab_closed(root: &Path) {
    let shown = with_columns(|columns| columns.shows_changes(root)).unwrap_or(false);
    if !shown && crate::app().changes.get().is_some() {
        service().change(|engine| engine.close_view(root));
    }
}

/// A closed tab came back: when it is a Changes tab, its root is shown again.
pub(crate) fn tab_reopened() {
    if let Some(root) = shown_root() {
        service().open_view(&root);
    }
}

/// Goes up whenever the tab's list or diff may read differently. The shell loads again when
/// it differs from the value it last drew.
#[no_mangle]
pub extern "C" fn nr_changes_revision() -> u64 {
    service().read(Engine::revision)
}

// MARK: Words

/// Every fixed word the tab draws besides its tools' tooltips and its picker's.
#[repr(C)]
pub struct NrChangesWords {
    /// After a file's name line when its diff gave up.
    pub whole: NrText,
    /// A row's ending for a file without a line diff: new, changed or deleted.
    pub new: NrText,
    pub changed: NrText,
    pub deleted: NrText,
    /// What happened, in the section of a file without diff lines.
    pub file_added: NrText,
    pub file_modified: NrText,
    pub file_deleted: NrText,
}

fn say(word: strings::Key) -> NrText {
    NrText::borrowing(strings::text(interface_language(), word))
}

/// The words the tab draws, in the interface language.
#[no_mangle]
pub extern "C" fn nr_changes_words() -> NrChangesWords {
    NrChangesWords {
        whole: say(key::CHANGES_WHOLE),
        new: say(key::CHANGES_NEW),
        changed: say(key::CHANGES_CHANGED),
        deleted: say(key::CHANGES_DELETED),
        file_added: say(key::CHANGES_FILE_ADDED),
        file_modified: say(key::CHANGES_FILE_MODIFIED),
        file_deleted: say(key::CHANGES_FILE_DELETED),
    }
}

// MARK: The files and their diff

/// A listed file and its diff, kept so the getters can lend its text.
struct Section {
    /// Names what it shows for the shell's caches; the same until its change differs.
    key: u64,
    row: FileRow,
    shown: view::Shown,
    rows: Vec<diff::Row>,
    headers: Vec<String>,
    /// Each hunk line's words as (first, count) in `words`, by hunk then line.
    line_words: Vec<Vec<(usize, usize)>>,
    words: Vec<NrChangesRange>,
    name: String,
    extension: String,
    /// Starts collapsed as noise (`view::starts_folded`).
    noisy: bool,
    /// Shows collapsed, only its name line (`view::folded`).
    folded: bool,
    /// One line-number column's width (`frames::gutter`).
    gutter: f64,
    /// Where each row wraps (`diff::wrap`), as (first, count) in `breaks`, and the visual line
    /// each row starts on; laid out for `wrapped_at` columns.
    row_breaks: Vec<(usize, usize)>,
    breaks: Vec<usize>,
    row_lines: Vec<usize>,
    lines: usize,
    wrapped_at: Option<usize>,
}

impl Section {
    /// `row`'s diff, read from the kept texts.
    fn of(row: FileRow, key: u64) -> Self {
        let service = service();
        let shown =
            view::show(service.store(), &row.change, service.clock()).unwrap_or_else(|error| {
                eprintln!("the changes could not show {}: {error}", row.change.path);
                view::Shown::default()
            });
        let (line_words, words) = flatten_words(&shown.diff);
        let rows = diff::rows(&shown.diff);
        let counts = row.change.is_text().then(|| shown.diff.counts());
        Self {
            key,
            noisy: view::starts_folded(&row, counts, !rows.is_empty()),
            folded: false,
            gutter: 0.0,
            row_breaks: Vec::new(),
            breaks: Vec::new(),
            row_lines: Vec::new(),
            lines: rows.len(),
            wrapped_at: None,
            rows,
            headers: shown.diff.hunks.iter().map(diff::Hunk::header).collect(),
            line_words,
            words,
            name: file_name(&row.change.path).to_owned(),
            extension: file_extension(&row.change.path).to_owned(),
            row,
            shown,
        }
    }

    /// Without diff lines (a picture, a binary, an empty text) it shows its name and what
    /// happened, and can be opened unless it is gone.
    fn openable(&self) -> bool {
        self.rows.is_empty() && self.row.status != Status::Deleted
    }

    /// Its name line's tools, left to right: while the pointer is over the line (`hovered`),
    /// Keep where `scope` keeps and Undo where it undoes (`hover_tools`); then Open for a file
    /// without diff lines that is still there, last so it keeps its place as the others come
    /// and go.
    fn line_tools(&self, hovered: bool, scope: Scope) -> Vec<NrChangesTool> {
        let shown = match hovered {
            true => hover_tools(scope),
            false => Vec::new(),
        };
        let open = self.openable().then_some(NrChangesTool::Open);
        shown.into_iter().chain(open).collect()
    }

    /// Takes `row`, counting its lines from the diff until the core's count comes.
    fn take(&mut self, mut row: FileRow) {
        if row.counts.is_none() && row.change.is_text() {
            row.counts = Some(self.shown.diff.counts());
        }
        self.row = row;
    }

    fn height(&self) -> f64 {
        frames::section_height(self.lines, self.folded)
    }

    /// The text row `row` draws, before its tabs are expanded.
    fn row_text(&self, row: diff::Row) -> &str {
        match row.cell {
            Cell::Header => &self.headers[row.hunk],
            Cell::Line(line) => self.shown.diff.hunks[row.hunk].lines[line].shown(),
        }
    }

    /// Wraps its rows to fit a diff `width` wide in a monospace face whose figures are
    /// `advance` wide (`diff::wrap`), unless already wrapped so.
    fn wrap(&mut self, width: f64, advance: f64) {
        let highest = self.highest();
        self.gutter = frames::gutter(highest, advance);
        let columns = frames::text_columns(width, self.gutter, advance);
        if self.wrapped_at == Some(columns) {
            return;
        }
        let tab = crate::window::SOURCE_TAB_SPACES as usize;
        let (mut row_breaks, mut breaks, mut row_lines) = (Vec::new(), Vec::new(), Vec::new());
        let mut line = 0;
        for row in &self.rows {
            let starts = match columns {
                usize::MAX => vec![0],
                columns => diff::wrap(self.row_text(*row), tab, columns),
            };
            row_lines.push(line);
            line += starts.len();
            row_breaks.push((breaks.len(), starts.len()));
            breaks.extend(starts);
        }
        self.lines = line;
        self.row_breaks = row_breaks;
        self.breaks = breaks;
        self.row_lines = row_lines;
        self.wrapped_at = Some(columns);
    }

    /// Its highest line number on either side.
    fn highest(&self) -> u32 {
        self.shown
            .diff
            .hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .flat_map(|line| [line.old, line.new])
            .flatten()
            .max()
            .unwrap_or(0) as u32
    }
}

/// Where a hunk sits down the diff: its file and its index there, and from its header's top to
/// its last row's bottom.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HunkPlace {
    file: usize,
    hunk: usize,
    top: f64,
    bottom: f64,
}

/// What `nr_changes_load` listed: the scope shown and its picker's words, every file's section,
/// where each starts and each hunk sits, and the file in view at the diff's last reported
/// offset.
struct Loaded {
    root: Option<PathBuf>,
    scope: Scope,
    /// The picker's words ("New · 4") and what the scope says with nothing listed.
    picker: String,
    empty: String,
    files: Vec<Section>,
    sections: Sections,
    /// Every shown hunk, in order down the diff; a collapsed file's are passed over.
    hunks: Vec<HunkPlace>,
    /// The hunk N or P last stepped to, and the offset that showed it; it stays current while
    /// the diff is there.
    stepped: Option<(usize, f64)>,
    in_view: Option<usize>,
    offset: f64,
    keys: u64,
    /// The tab's parts as last laid out (`nr_changes_frames`), for hit-testing.
    laid: frames::Frames,
    /// How wide the shell's monospace figures are, which rows wrap by; 0 until measured.
    advance: f64,
}

impl Loaded {
    /// Lists `rows` of `root`, keeping the diffs of files whose change is the same, and lays
    /// them out as `view` folds them.
    fn reload(&mut self, root: Option<PathBuf>, rows: Vec<FileRow>, view: Option<&ViewState>) {
        let mut kept = std::mem::take(&mut self.files);
        if self.root != root {
            kept.clear();
        }
        let mut files = Vec::with_capacity(rows.len());
        for row in rows {
            let same = kept.iter().position(|kept| kept.row.change == row.change);
            let mut file = match same {
                Some(at) => kept.swap_remove(at),
                None => {
                    self.keys += 1;
                    Section::of(row.clone(), self.keys)
                }
            };
            file.take(row);
            files.push(file);
        }
        self.files = files;
        self.root = root;
        self.lay(view);
    }

    /// Folds each file as `view` says and lays the sections out down the diff.
    fn lay(&mut self, view: Option<&ViewState>) {
        for file in &mut self.files {
            file.folded = view::folded(view, &file.row, file.noisy);
            file.wrap(self.laid.diff.width, self.advance);
        }
        self.sections = Sections::new(self.files.iter().map(Section::height));
        self.hunks = hunk_places(&self.files, &self.sections);
        self.stepped = None;
    }

    /// The tools of file `index`'s name line, with the pointer over it or not.
    fn line_tools(&self, index: usize, hovered: bool) -> Vec<NrChangesTool> {
        self.files
            .get(index)
            .map_or_else(Vec::new, |file| file.line_tools(hovered, self.scope))
    }

    fn position(&self, path: &str) -> Option<usize> {
        self.files
            .iter()
            .position(|file| file.row.change.path == path)
    }

    /// Brings file `index` into view, `within` points into its section, and remembers it in
    /// the tab's view. Returns the offset that shows it so.
    fn bring_into_view(&mut self, index: usize, within: f64) -> Option<f64> {
        let offset = self.sections.anchor(index, within)?;
        self.in_view = Some(index);
        self.offset = offset;
        let path = self.files[index].row.change.path.clone();
        if let Some(root) = &self.root {
            service().set_view(root, |view| {
                view.selected = Some(path);
                view.within = within;
            });
        }
        Some(offset)
    }

    /// The hunk N or P stepped to, while the diff is still where it showed it.
    fn stepped_now(&self) -> Option<usize> {
        const SLACK: f64 = 1.0;
        self.stepped
            .filter(|(_, at)| (at - self.offset).abs() <= SLACK)
            .map(|(hunk, _)| hunk)
    }

    /// The hunk the keys act on: the one N or P stepped to while the diff is still there, else
    /// the first whose rows show below the pinned name line.
    fn current_hunk(&self) -> Option<usize> {
        let shown = self.offset + self.laid.diff.height;
        self.stepped_now().or_else(|| {
            self.hunks.iter().position(|place| {
                place.bottom > self.offset + frames::SEPARATOR && place.top < shown
            })
        })
    }

    /// The hunk whose rows hold point `y` of the tab with the diff scrolled to `offset`.
    fn hunk_at(&self, offset: f64, y: f64) -> Option<usize> {
        let down = offset + y - self.laid.diff.y;
        self.hunks
            .iter()
            .position(|place| place.top <= down && down < place.bottom)
    }

    /// Hunk `index`'s strip with the diff scrolled to `offset`: the strip and each tool with its
    /// frame; `None` for a scope without hunk tools.
    fn strip(&self, index: usize, offset: f64) -> Option<(Frame, Vec<(NrChangesTool, Frame)>)> {
        let place = self.hunks.get(index)?;
        let tools = hover_tools(self.scope);
        if tools.is_empty() {
            return None;
        }
        let diff = self.laid.diff;
        let floor = self
            .sections
            .pinned(offset)
            .map_or(0.0, |(_, top)| top + frames::SEPARATOR);
        let row = frames::strip_row(
            diff.y + place.top - offset,
            diff.y + place.bottom - offset,
            diff.y + floor,
        );
        let (strip, placed) = frames::strip(row, frames::tools_end(diff), tools.len());
        Some((strip, tools.into_iter().zip(placed).collect()))
    }

    /// What point (`x`, `y`) over a hunk is on: one of its strip's tools, else the hunk.
    fn hunk_hit(&self, offset: f64, (x, y): (f64, f64)) -> Option<Found> {
        if !frames::inside(&self.laid.diff, x, y) {
            return None;
        }
        let index = self.hunk_at(offset, y)?;
        let tool = self.strip(index, offset).and_then(|(_, tools)| {
            tools
                .into_iter()
                .find(|(_, frame)| frames::inside(frame, x, y))
        });
        Some(match tool {
            Some((tool, frame)) => Found::HunkTool(index, tool, frame),
            None => Found::Hunk(index),
        })
    }
}

type Frame = crate::board::Frame;

/// What a point is on (`nr_changes_hit`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Found {
    LineTool(usize, NrChangesTool, Frame),
    Line(usize, Frame),
    HunkTool(usize, NrChangesTool, Frame),
    Hunk(usize),
}

static LOADED: Mutex<Loaded> = Mutex::new(Loaded {
    root: None,
    scope: Scope::New,
    picker: String::new(),
    empty: String::new(),
    files: Vec::new(),
    sections: Sections::empty(),
    hunks: Vec::new(),
    stepped: None,
    in_view: None,
    offset: 0.0,
    keys: 0,
    laid: frames::Frames {
        picker: frames::NOWHERE,
        list: frames::NOWHERE,
        diff: frames::NOWHERE,
    },
    advance: 0.0,
});

/// Where each hunk of `files` sits down the diff laid out as `sections`; a collapsed file's are
/// passed over.
fn hunk_places(files: &[Section], sections: &Sections) -> Vec<HunkPlace> {
    let mut places = Vec::new();
    for (index, file) in files.iter().enumerate() {
        if file.folded {
            continue;
        }
        let first = sections.top(index).unwrap_or(0.0) + frames::SEPARATOR;
        let line = |at: usize| file.row_lines.get(at).copied().unwrap_or(at);
        let down = |line: usize| first + line as f64 * frames::DIFF_LINE;
        let headers: Vec<(usize, usize)> = file
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| row.cell == Cell::Header)
            .map(|(at, row)| (at, row.hunk))
            .collect();
        for (number, (at, hunk)) in headers.iter().enumerate() {
            let end = headers
                .get(number + 1)
                .map_or(file.lines, |(next, _)| line(*next));
            places.push(HunkPlace {
                file: index,
                hunk: *hunk,
                top: down(line(*at)),
                bottom: down(end),
            });
        }
    }
    places
}

/// What `nr_changes_load` found: how many files, the one in view (-1 with none), and the
/// offset that keeps it where it was, as far into its section as before.
#[repr(C)]
pub struct NrChangesLoaded {
    pub files: usize,
    pub in_view: i64,
    pub offset: f64,
}

/// The scope `root`'s tab shows, its picker's words for `count` files, and what it says with
/// none.
fn scope_words(engine: &Engine, root: &Path, count: usize) -> (Scope, String, String) {
    let language = interface_language();
    let Some(watched) = engine.roots.get(root) else {
        return (Scope::New, String::new(), String::new());
    };
    let scope = view::shown_scope(watched);
    let branch = view::branch_name(watched).unwrap_or_default();
    let picker = strings::format(
        language,
        key::CHANGES_PICKER,
        &[
            ("scope", &scope_name(scope, branch, language)),
            ("count", &count.to_string()),
        ],
    );
    let empty = match scope {
        Scope::New => strings::text(language, key::CHANGES_EMPTY).to_owned(),
        Scope::Uncommitted => strings::text(language, key::CHANGES_EMPTY_UNCOMMITTED).to_owned(),
        Scope::Branch => strings::format(language, key::CHANGES_EMPTY_BRANCH, &[("name", branch)]),
    };
    (scope, picker, empty)
}

/// What the picker calls `scope`; vs main by the default branch's name, `branch`.
fn scope_name(scope: Scope, branch: &str, language: &str) -> String {
    match scope {
        Scope::New => strings::text(language, key::CHANGES_SCOPE_NEW).to_owned(),
        Scope::Uncommitted => strings::text(language, key::CHANGES_SCOPE_UNCOMMITTED).to_owned(),
        Scope::Branch => strings::format(language, key::CHANGES_SCOPE_BRANCH, &[("name", branch)]),
    }
}

/// Lists the active Changes tab's files and their diffs, in the scope it shows.
#[no_mangle]
pub extern "C" fn nr_changes_load() -> NrChangesLoaded {
    let root = shown_root();
    let (rows, in_view, within, view, words) = service().read(|engine| {
        let Some(root) = root.as_deref() else {
            let words = (Scope::New, String::new(), String::new());
            return (Vec::new(), None, 0.0, None, words);
        };
        let rows = view::list(engine, root);
        let in_view = view::shown_selection(engine, root, &rows);
        let view = engine.views.get(root).cloned();
        let within = view
            .as_ref()
            .filter(|view| view.selected == in_view)
            .map_or(0.0, |view| view.within);
        let words = scope_words(engine, root, rows.len());
        (rows, in_view, within, view, words)
    });
    let mut loaded = LOADED.relock();
    (loaded.scope, loaded.picker, loaded.empty) = words;
    loaded.reload(root, rows, view.as_ref());
    let index = in_view.and_then(|path| loaded.position(&path));
    loaded.in_view = index;
    let offset = index.and_then(|index| loaded.sections.anchor(index, within));
    loaded.offset = offset.unwrap_or(0.0);
    NrChangesLoaded {
        files: loaded.files.len(),
        in_view: index.map_or(-1, |index| index as i64),
        offset: loaded.offset,
    }
}

/// The picker's words as last loaded ("New · 4", "vs main · 2") and what the scope says with
/// nothing listed ("No uncommitted changes.").
#[repr(C)]
pub struct NrChangesPicker {
    pub label: NrText,
    pub empty: NrText,
}

#[no_mangle]
pub extern "C" fn nr_changes_picker() -> NrChangesPicker {
    let loaded = LOADED.relock();
    NrChangesPicker {
        label: NrText::borrowing(&loaded.picker),
        empty: NrText::borrowing(&loaded.empty),
    }
}

/// What happened to a file.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrChangesStatus {
    Added,
    Modified,
    Deleted,
}

fn status_of(status: Status) -> NrChangesStatus {
    match status {
        Status::Added => NrChangesStatus::Added,
        Status::Modified => NrChangesStatus::Modified,
        Status::Deleted => NrChangesStatus::Deleted,
    }
}

/// One listed file: its row in the list and its section of the diff. `path` is in the root;
/// a file with a line diff (`text`) ends its row in `+plus −minus` once `counted`, a new or
/// deleted one or one without a line diff in its status's word. Its section starts `top` down
/// the diff and is `height` tall: the name line, then `rows` drawn rows
/// (`nr_changes_diff_row`) over `lines` visual lines once wrapped, their numbers in two columns
/// `gutter` wide, or with none its name and what happened; `folded`, only the name
/// line, which ends in the row's ending. `whole` says it was too different to compare line by
/// line. `key` is the same while its diff is.
#[repr(C)]
pub struct NrChangesFile {
    pub key: u64,
    pub path: NrText,
    pub name: NrText,
    pub status: NrChangesStatus,
    pub text: bool,
    pub counted: bool,
    pub plus: u32,
    pub minus: u32,
    /// Two terminals changed it at the same time.
    pub contested: bool,
    pub in_view: bool,
    pub whole: bool,
    pub folded: bool,
    pub rows: usize,
    pub lines: usize,
    pub gutter: f64,
    pub top: f64,
    pub height: f64,
}

/// Fills `out` with file `index` as last loaded; false past the last.
///
/// # Safety
/// `out` must point to writable storage for one `NrChangesFile`.
#[no_mangle]
pub unsafe extern "C" fn nr_changes_file(index: usize, out: *mut NrChangesFile) -> bool {
    let loaded = LOADED.relock();
    let Some(file) = loaded.files.get(index) else {
        return false;
    };
    let (plus, minus) = file.row.counts.unwrap_or((0, 0));
    let filled = NrChangesFile {
        key: file.key,
        path: NrText::borrowing(&file.row.change.path),
        name: NrText::borrowing(&file.name),
        status: status_of(file.row.status),
        text: file.row.change.is_text(),
        counted: file.row.counts.is_some(),
        plus: plus as u32,
        minus: minus as u32,
        contested: file.row.contested,
        in_view: loaded.in_view == Some(index),
        whole: file.shown.diff.whole,
        folded: file.folded,
        rows: file.rows.len(),
        lines: file.lines,
        gutter: file.gutter,
        top: loaded.sections.top(index).unwrap_or(0.0),
        height: loaded.sections.height(index).unwrap_or(0.0),
    };
    // SAFETY: the caller passes storage for one file.
    unsafe { out.write(filled) };
    true
}

/// File `index`'s extension and its whole text on each side, for syntax colours
/// (`nr_syntax_call`): a row's line numbers count lines in them from 1. Empty past the last.
#[repr(C)]
pub struct NrChangesTexts {
    pub extension: NrText,
    pub before_text: NrText,
    pub after_text: NrText,
}

#[no_mangle]
pub extern "C" fn nr_changes_texts(index: usize) -> NrChangesTexts {
    let loaded = LOADED.relock();
    let lend = |text: Option<&str>| NrText::borrowing(text.unwrap_or(""));
    let file = loaded.files.get(index);
    NrChangesTexts {
        extension: lend(file.map(|file| file.extension.as_str())),
        before_text: lend(file.map(|file| file.shown.before_text.as_str())),
        after_text: lend(file.map(|file| file.shown.after_text.as_str())),
    }
}

/// The extension of the file at `path` (relative, `/` between names).
fn file_extension(path: &str) -> &str {
    crate::list::name_extension(file_name(path))
}

/// The last part of `path` (relative, `/` between names).
fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Every hunk line's changed words, flattened.
fn flatten_words(diff: &FileDiff) -> (Vec<Vec<(usize, usize)>>, Vec<NrChangesRange>) {
    let mut words = Vec::new();
    let line_words = diff
        .hunks
        .iter()
        .map(|hunk| {
            hunk.lines
                .iter()
                .map(|line| {
                    let first = words.len();
                    words.extend(line.words.iter().map(|range| NrChangesRange {
                        start: range.start,
                        end: range.end,
                    }));
                    (first, line.words.len())
                })
                .collect()
        })
        .collect();
    (line_words, words)
}

/// What a diff row shows.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrChangesRowShows {
    /// The hunk's header, `@@ -a,b +c,d @@`.
    Header,
    Context,
    Added,
    Removed,
}

/// A byte range of a row's text.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NrChangesRange {
    pub start: usize,
    pub end: usize,
}

/// One drawn row of a file's diff. Line numbers are 1-based, 0 for none. Its changed words are
/// `nr_changes_word(file, word_first + n)` for n below `word_count`, as byte ranges of `text`.
/// It starts on visual line `line` of its file's diff and wraps onto `break_count` lines, each
/// starting at `nr_changes_break(file, break_first + n)`: a character index into `text` with
/// its tabs expanded (`diff::wrap`).
#[repr(C)]
pub struct NrChangesRow {
    pub line: usize,
    pub break_first: usize,
    pub break_count: usize,
    pub hunk: usize,
    pub shows: NrChangesRowShows,
    pub old_line: u32,
    pub new_line: u32,
    pub text: NrText,
    pub word_first: usize,
    pub word_count: usize,
}

fn row_of(file: &Section, index: usize, row: diff::Row) -> NrChangesRow {
    let (break_first, break_count) = file.row_breaks.get(index).copied().unwrap_or((0, 0));
    let header = NrChangesRow {
        line: file.row_lines.get(index).copied().unwrap_or(index),
        break_first,
        break_count,
        hunk: row.hunk,
        shows: NrChangesRowShows::Header,
        old_line: 0,
        new_line: 0,
        text: NrText::borrowing(&file.headers[row.hunk]),
        word_first: 0,
        word_count: 0,
    };
    let Cell::Line(line) = row.cell else {
        return header;
    };
    let shown = &file.shown.diff.hunks[row.hunk].lines[line];
    let (word_first, word_count) = file.line_words[row.hunk][line];
    NrChangesRow {
        shows: match shown.kind {
            LineKind::Context => NrChangesRowShows::Context,
            LineKind::Added => NrChangesRowShows::Added,
            LineKind::Removed => NrChangesRowShows::Removed,
        },
        old_line: shown.old.map_or(0, |line| line as u32),
        new_line: shown.new.map_or(0, |line| line as u32),
        text: NrText::borrowing(shown.shown()),
        word_first,
        word_count,
        ..header
    }
}

/// Fills `out` with drawn row `index` of file `file` as last loaded; false past the last.
///
/// # Safety
/// `out` must point to writable storage for one `NrChangesRow`.
#[no_mangle]
pub unsafe extern "C" fn nr_changes_diff_row(
    file: usize,
    index: usize,
    out: *mut NrChangesRow,
) -> bool {
    let loaded = LOADED.relock();
    let Some((file, row)) = loaded
        .files
        .get(file)
        .and_then(|file| Some((file, *file.rows.get(index)?)))
    else {
        return false;
    };
    // SAFETY: the caller passes storage for one row.
    unsafe { out.write(row_of(file, index, row)) };
    true
}

/// Where a wrapped visual line starts (`NrChangesRow::break_first`): break `index` of file
/// `file` as last loaded; 0 past the last.
#[no_mangle]
pub extern "C" fn nr_changes_break(file: usize, index: usize) -> usize {
    let loaded = LOADED.relock();
    loaded
        .files
        .get(file)
        .and_then(|file| file.breaks.get(index).copied())
        .unwrap_or(0)
}

/// Changed word `index` of file `file` as last loaded; an empty range past the last.
#[no_mangle]
pub extern "C" fn nr_changes_word(file: usize, index: usize) -> NrChangesRange {
    let loaded = LOADED.relock();
    loaded
        .files
        .get(file)
        .and_then(|file| file.words.get(index).copied())
        .unwrap_or(NrChangesRange { start: 0, end: 0 })
}

// MARK: Scrolling

/// The diff is scrolled to `offset`: the file whose section holds its top edge comes into
/// view (it lights in the list, its name line is pinned, and X acts on it). Returns that file,
/// -1 with none.
#[no_mangle]
pub extern "C" fn nr_changes_scrolled(offset: f64) -> i64 {
    let mut loaded = LOADED.relock();
    let Some(index) = loaded.sections.in_view(offset) else {
        return -1;
    };
    let within = offset - loaded.sections.top(index).unwrap_or(0.0);
    loaded.bring_into_view(index, within);
    loaded.offset = offset;
    index as i64
}

/// Brings file `index` into view at its top, as a click on it in the list does, expanding it
/// when it is collapsed: Scroll to `offset`, or Changed when it expanded, to load again, which
/// scrolls there. Past the last, Scroll where the diff is.
#[no_mangle]
pub extern "C" fn nr_changes_select(index: usize) -> NrChangesKey {
    let mut loaded = LOADED.relock();
    let Some(file) = loaded.files.get(index) else {
        return scroll_to(loaded.offset);
    };
    let expand = file.folded.then(|| file.row.change.path.clone());
    if let Some(path) = &expand {
        refold(&mut loaded, |view| {
            view.folds.insert(path.clone(), false);
        });
    }
    let offset = loaded.bring_into_view(index, 0.0).unwrap_or(loaded.offset);
    match expand {
        Some(_) => answer(NrChangesKeyEffect::Changed, offset),
        None => scroll_to(offset),
    }
}

fn scroll_to(offset: f64) -> NrChangesKey {
    answer(NrChangesKeyEffect::Scroll, offset)
}

/// An answer to the shell that only loads again or scrolls.
fn answer(effect: NrChangesKeyEffect, offset: f64) -> NrChangesKey {
    NrChangesKey {
        effect,
        offset,
        said: crate::menus::ffi::nothing(),
    }
}

/// The name line pinned at the diff's top: `file` (-1 with none) and its `top` in the tab's
/// points, 0 down to a line's height above the diff while the next file's line pushes it out.
/// `scrolled` says the diff is scrolled into its section, so it is drawn over the rows under
/// it; otherwise it is where its own line is.
#[repr(C)]
pub struct NrChangesPinned {
    pub file: i64,
    pub top: f64,
    pub scrolled: bool,
}

/// The pinned name line at scroll `offset` (`Sections::pinned`).
#[no_mangle]
pub extern "C" fn nr_changes_pinned(offset: f64) -> NrChangesPinned {
    let loaded = LOADED.relock();
    let pinned = loaded.sections.pinned(offset);
    let scrolled = pinned
        .and_then(|(index, _)| loaded.sections.top(index))
        .is_some_and(|top| offset > top);
    NrChangesPinned {
        file: pinned.map_or(-1, |(index, _)| index as i64),
        top: loaded.laid.diff.y + pinned.map_or(0.0, |(_, top)| top),
        scrolled,
    }
}

/// Collapses file `index` when it is expanded, else expands it, as a click on its name line
/// does; with `all` (⌥) every file goes the same way. The file in view keeps its name line
/// where it is on screen (`Sections::carry`). False past the last; else load again.
#[no_mangle]
pub extern "C" fn nr_changes_fold(index: usize, all: bool) -> bool {
    fold(&mut LOADED.relock(), index, all)
}

fn fold(loaded: &mut Loaded, index: usize, all: bool) -> bool {
    let Some(file) = loaded.files.get(index) else {
        return false;
    };
    let folded = !file.folded;
    let paths: Vec<String> = match all {
        true => loaded
            .files
            .iter()
            .map(|file| file.row.change.path.clone())
            .collect(),
        false => vec![file.row.change.path.clone()],
    };
    refold(loaded, |view| {
        view.folds
            .extend(paths.into_iter().map(|path| (path, folded)));
    });
    true
}

/// Changes how the active tab folds its files with `change`, lays them out again and keeps
/// the file in view where it was on screen (`Sections::carry`).
fn refold(loaded: &mut Loaded, change: impl FnOnce(&mut ViewState)) {
    let Some(root) = loaded.root.clone() else {
        return;
    };
    let view = service().change(|engine| {
        let view = engine.views.get_mut(&root).map(|view| {
            change(view);
            view.clone()
        });
        engine.touch();
        view
    });
    let before = loaded.sections.clone();
    loaded.lay(view.as_ref());
    if let Some((index, within)) = before.carry(&loaded.sections, loaded.offset) {
        loaded.bring_into_view(index, within);
    }
}

/// Lays the active tab's files out again as its view folds them, keeping the file in view where
/// it was on screen (`Sections::carry`).
fn relay(loaded: &mut Loaded) {
    let view = loaded
        .root
        .clone()
        .and_then(|root| service().read(|engine| engine.views.get(&root).cloned()));
    let before = loaded.sections.clone();
    loaded.lay(view.as_ref());
    if let Some((index, within)) = before.carry(&loaded.sections, loaded.offset) {
        loaded.bring_into_view(index, within);
    }
}

/// The offset of the hunk after the diff's top edge (before it when `step` is negative), and
/// its index; `None` past the ends.
fn stepped_hunk(hunks: &[HunkPlace], offset: f64, step: i64) -> Option<(usize, f64)> {
    const SLACK: f64 = 0.5;
    let found = match step < 0 {
        true => hunks.iter().rposition(|place| place.top < offset - SLACK),
        false => hunks.iter().position(|place| place.top > offset + SLACK),
    };
    found.map(|index| (index, hunks[index].top))
}

// MARK: Keys

/// What a key did in the tab.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrChangesKeyEffect {
    /// Not the tab's key; it passes on.
    PassOn,
    /// Redraw: load again (`nr_changes_load`).
    Changed,
    /// Scroll the diff to `offset`.
    Scroll,
}

/// What a key or a click in the list asks of the shell, and `said`, what an action came to
/// for the window to carry out (a notice of why it failed), else nothing.
#[repr(C)]
pub struct NrChangesKey {
    pub effect: NrChangesKeyEffect,
    pub offset: f64,
    pub said: NrMenuEffect,
}

/// What Keep and Undo act on from the keys: the current hunk, or the file in view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Hunk,
    File,
}

/// What a key asks of the tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asked {
    File(i64),
    Hunk(i64),
    /// Collapse or expand the file in view, or with `true` every file.
    Fold(bool),
    Scope(Scope),
    Keep(Reach),
    Undo(Reach),
    /// Put the last Undo back.
    Restore,
}

/// J and K or Down and Up step from file to file, N and P from hunk to hunk, and X collapses or
/// expands the file in view, ⌥X every file; the ⌘ keys are `commanded`. Other keys with ⌘,
/// Control or Option held pass on.
fn asked(press: &NrKeyPress) -> Option<Asked> {
    use crate::ffi::{NR_HELD_COMMAND, NR_HELD_CONTROL, NR_HELD_OPTION};
    if press.held & NR_HELD_CONTROL != 0 {
        return None;
    }
    if press.held & NR_HELD_COMMAND != 0 {
        return commanded(press);
    }
    if press.letter() == Some('x') {
        return Some(Asked::Fold(press.held & NR_HELD_OPTION != 0));
    }
    if press.held & NR_HELD_OPTION != 0 {
        return None;
    }
    let letter = char::from_u32(press.character).map(|typed| typed.to_ascii_lowercase());
    match (press.key, letter) {
        (NrKey::Down, _) | (NrKey::Other, Some('j')) => Some(Asked::File(1)),
        (NrKey::Up, _) | (NrKey::Other, Some('k')) => Some(Asked::File(-1)),
        (NrKey::Other, Some('n')) => Some(Asked::Hunk(1)),
        (NrKey::Other, Some('p')) => Some(Asked::Hunk(-1)),
        _ => None,
    }
}

/// ⌘1, ⌘2 and ⌘3 show New, Uncommitted and vs main; ⌘Y keeps the current hunk and ⌘⇧Y the file
/// in view; ⌘⌥Z undoes the current hunk and ⌘⇧⌥Z the file in view; ⌘Z puts the last Undo back.
/// Other ⌘ keys pass on (⌘X stays Cut).
fn commanded(press: &NrKeyPress) -> Option<Asked> {
    use crate::ffi::{NR_HELD_OPTION, NR_HELD_SHIFT};
    let shift = press.held & NR_HELD_SHIFT != 0;
    let option = press.held & NR_HELD_OPTION != 0;
    let reach = match shift {
        true => Reach::File,
        false => Reach::Hunk,
    };
    match (press.letter()?, option) {
        ('1', false) if !shift => Some(Asked::Scope(Scope::New)),
        ('2', false) if !shift => Some(Asked::Scope(Scope::Uncommitted)),
        ('3', false) if !shift => Some(Asked::Scope(Scope::Branch)),
        ('y', false) => Some(Asked::Keep(reach)),
        ('z', true) => Some(Asked::Undo(reach)),
        ('z', false) if !shift => Some(Asked::Restore),
        _ => None,
    }
}

/// Does what `press` asks of the tab (`asked`).
#[no_mangle]
pub extern "C" fn nr_changes_key(press: NrKeyPress) -> NrChangesKey {
    let Some(asked) = asked(&press) else {
        return answer(NrChangesKeyEffect::PassOn, 0.0);
    };
    match asked {
        Asked::File(_) | Asked::Hunk(_) | Asked::Fold(_) => step(asked),
        Asked::Scope(scope) => acted_by_key(choose_scope(scope)),
        Asked::Keep(reach) | Asked::Undo(reach) => acted_by_key(act_by_key(asked, reach)),
        Asked::Restore if nothing_to_restore() => answer(NrChangesKeyEffect::PassOn, 0.0),
        Asked::Restore => acted_by_key(restore_last()),
    }
}

/// Steps, scrolls or folds as `asked` says.
fn step(asked: Asked) -> NrChangesKey {
    let mut loaded = LOADED.relock();
    if let Asked::Fold(all) = asked {
        let folded = loaded
            .in_view
            .is_some_and(|index| fold(&mut loaded, index, all));
        let effect = match folded {
            true => NrChangesKeyEffect::Changed,
            false => NrChangesKeyEffect::Scroll,
        };
        return answer(effect, loaded.offset);
    }
    let offset = match asked {
        Asked::File(step) => view::stepped_file(loaded.files.len(), loaded.in_view, step)
            .and_then(|index| loaded.bring_into_view(index, 0.0)),
        Asked::Hunk(step) => stepped_hunk(&loaded.hunks, loaded.offset, step).map(|stepped| {
            loaded.stepped = Some(stepped);
            stepped.1
        }),
        Asked::Fold(_) | Asked::Scope(_) | Asked::Keep(_) | Asked::Undo(_) | Asked::Restore => None,
    };
    scroll_to(offset.unwrap_or(loaded.offset))
}

/// Keep or Undo (`asked`) of the current hunk or the file in view.
fn act_by_key(asked: Asked, reach: Reach) -> Outcome {
    let target = {
        let loaded = LOADED.relock();
        match reach {
            Reach::Hunk => loaded.current_hunk(),
            Reach::File => loaded.in_view,
        }
    };
    let Some(target) = target else {
        return Ok(false);
    };
    let tool = match asked {
        Asked::Undo(_) => NrChangesTool::Undo,
        _ => NrChangesTool::Keep,
    };
    match reach {
        Reach::Hunk => use_hunk_tool(target, tool),
        Reach::File => use_file_tool(target, tool),
    }
}

/// A key's action came to `outcome`: load again, or stay, and say why it failed.
fn acted_by_key(outcome: Outcome) -> NrChangesKey {
    let offset = LOADED.relock().offset;
    let effect = match &outcome {
        Ok(true) => NrChangesKeyEffect::Changed,
        Ok(false) | Err(_) => NrChangesKeyEffect::Scroll,
    };
    NrChangesKey {
        effect,
        offset,
        said: match outcome {
            Err(failed) => crate::menus::ffi::failed(failed.title, failed.words),
            Ok(_) => crate::menus::ffi::nothing(),
        },
    }
}

// MARK: Actions

/// Why an action failed: a notice's title and its words.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Failed {
    pub(crate) title: &'static str,
    pub(crate) words: String,
}

/// What an action on the active tab came to: whether to load again, or why it failed.
pub(crate) type Outcome = Result<bool, Failed>;

/// An Undo's failure, in words.
fn undo_failed(error: &ActionError) -> Failed {
    let language = interface_language();
    Failed {
        title: strings::text(language, key::CHANGES_UNDO_FAILED),
        words: error.say(language),
    }
}

/// A Keep's failure, in words.
fn keep_failed(error: &ActionError) -> Failed {
    let language = interface_language();
    Failed {
        title: strings::text(language, key::CHANGES_KEEP_FAILED),
        words: error.say(language),
    }
}

/// Listed file `index` as last loaded: the active tab's root, its rows and that file's row.
fn listed(index: usize) -> Option<(PathBuf, Vec<FileRow>, FileRow)> {
    let path = LOADED.relock().files.get(index)?.row.change.path.clone();
    listed_at(&path)
}

/// The listed file at `path`: the active tab's root, its rows and that file's row.
fn listed_at(path: &str) -> Option<(PathBuf, Vec<FileRow>, FileRow)> {
    let root = shown_root()?;
    service().read(|engine| {
        let rows = view::list(engine, &root);
        let row = rows.iter().find(|row| row.change.path == path)?.clone();
        Some((root, rows, row))
    })
}

/// The scope `root`'s tab shows.
fn scope_of(root: &Path) -> Scope {
    service().read(|engine| engine.roots.get(root).map_or(Scope::New, view::shown_scope))
}

/// Brings the file after `row` into view once it leaves the list, at its top, so looking goes
/// on down the diff; only when it was the file in view.
fn move_past(root: &Path, rows: &[FileRow], row: &FileRow) {
    let in_view = service().read(|engine| view::shown_selection(engine, root, rows));
    if in_view.as_deref() != Some(row.change.path.as_str()) {
        return;
    }
    let next = view::next_after(rows, &row.change.path);
    service().change(|engine| {
        if let Some(view) = engine.views.get_mut(root) {
            view.selected = next;
            view.within = 0.0;
        }
        engine.touch();
    });
}

/// Shows `scope` in the active tab, when offered.
fn choose_scope(scope: Scope) -> Outcome {
    let Some(root) = shown_root() else {
        return Ok(false);
    };
    Ok(service().change(|engine| view::choose(engine, &root, scope)))
}

/// Runs `tool` on listed file `index`: Keep or Undo the whole file, or Open it.
fn use_file_tool(index: usize, tool: NrChangesTool) -> Outcome {
    let Some((root, rows, row)) = listed(index) else {
        return Ok(false);
    };
    let scope = scope_of(&root);
    match tool {
        NrChangesTool::Open => Ok(nr_changes_open_file(index)),
        NrChangesTool::Keep if scope.keeps() => {
            move_past(&root, &rows, &row);
            let now_ms = service().now_ms();
            Ok(service().change(|engine| view::keep_file(engine, &root, &row.change.path, now_ms)))
        }
        NrChangesTool::Undo if scope.undoes() => {
            let undone = with_trash(|trash| {
                actions::undo_file(&root, &row.change, service().store(), trash)
            });
            let undone = undone.map_err(|error| undo_failed(&error))?;
            move_past(&root, &rows, &row);
            remember_undone(&root, vec![undone]);
            Ok(true)
        }
        NrChangesTool::Keep | NrChangesTool::Undo => Ok(false),
    }
}

/// Runs `tool` on hunk `index` as last loaded (`Loaded::hunks`): Keep or Undo it.
fn use_hunk_tool(index: usize, tool: NrChangesTool) -> Outcome {
    let Some((root, path, hunk)) = hunk_of(index) else {
        return Ok(false);
    };
    let scope = scope_of(&root);
    let service = service();
    match tool {
        NrChangesTool::Keep if scope.keeps() => {
            let now_ms = service.now_ms();
            let kept = service.change(|engine| {
                let file = (root.as_path(), path.as_str());
                view::keep_hunk(engine, service.store(), service.clock(), file, hunk, now_ms)
            });
            kept.map_err(|error| keep_failed(&error))
        }
        NrChangesTool::Undo if scope.undoes() => {
            let Some((_, _, row)) = listed_at(&path) else {
                return Ok(false);
            };
            let shown = view::show(service.store(), &row.change, service.clock())
                .map_err(|error| undo_failed(&ActionError::from(error)))?;
            let Some(hunk) = shown.diff.hunks.get(hunk) else {
                return Ok(false);
            };
            let undone = with_trash(|trash| {
                actions::undo_hunk(&root, &row.change, hunk, service.store(), trash)
            });
            remember_undone(&root, vec![undone.map_err(|error| undo_failed(&error))?]);
            Ok(true)
        }
        NrChangesTool::Open | NrChangesTool::Keep | NrChangesTool::Undo => Ok(false),
    }
}

/// Hunk `index` as last loaded: the active tab's root, its file's path and its index there.
fn hunk_of(index: usize) -> Option<(PathBuf, String, usize)> {
    let loaded = LOADED.relock();
    let place = loaded.hunks.get(index)?;
    let path = loaded.files.get(place.file)?.row.change.path.clone();
    Some((loaded.root.clone()?, path, place.hunk))
}

/// Calls `act` with the drive's trash: the system's, a check's own folder, or none.
fn with_trash<T>(act: impl FnOnce(Option<actions::Trash>) -> T) -> T {
    let trash = crate::ffi::drive_trash();
    let trash_file = trash
        .as_ref()
        .map(|trash| move |path: &Path| -> std::io::Result<()> { trash(path).map(|_| ()) });
    act(trash_file.as_ref().map(|trash| trash as actions::Trash))
}

/// Keeps what an Undo in `root` did, for ⌘Z right after, and reads the root again.
fn remember_undone(root: &Path, undone: Vec<Undone>) {
    service().change(|engine| {
        engine.undone = undone;
        engine.refresh(root);
    });
}

/// Whether there is no Undo for ⌘Z to put back.
fn nothing_to_restore() -> bool {
    service().read(|engine| engine.undone.is_empty())
}

/// Puts the last Undo back, file by file in reverse, as ⌘Z right after it does, and brings
/// its file into view.
fn restore_last() -> Outcome {
    let undone = service().change(|engine| std::mem::take(&mut engine.undone));
    let restored = with_trash(|trash| {
        undone
            .iter()
            .rev()
            .try_for_each(|file| actions::restore(file, service().store(), trash))
    });
    if let Some(root) = shown_root() {
        let first = undone
            .first()
            .and_then(|file| relative_to(&root, &file.path));
        service().change(|engine| {
            // The file put back comes into view again, at its top.
            if let Some(view) = engine.views.get_mut(&root).filter(|_| first.is_some()) {
                view.selected = first;
                view.within = 0.0;
            }
            engine.refresh(&root);
        });
    }
    restored
        .map(|()| !undone.is_empty())
        .map_err(|error| undo_failed(&error))
}

/// Keeps every file New lists (Keep all).
fn keep_all() -> Outcome {
    let Some(root) = shown_root().filter(|root| scope_of(root).keeps()) else {
        return Ok(false);
    };
    let now_ms = service().now_ms();
    service().change(|engine| view::keep_all(engine, &root, now_ms));
    Ok(true)
}

/// Undoes every file New lists (Undo all), each kept in the trash first; ⌘Z puts them back.
fn undo_all() -> Outcome {
    let Some(root) = shown_root().filter(|root| scope_of(root) == Scope::New) else {
        return Ok(false);
    };
    let rows = service().read(|engine| view::list(engine, &root));
    let changes: Vec<_> = rows.into_iter().map(|row| row.change).collect();
    let (undone, done) =
        with_trash(|trash| actions::undo_files(&root, &changes, service().store(), trash));
    remember_undone(&root, undone);
    done.map(|()| true).map_err(|error| undo_failed(&error))
}

/// Runs `tool` of listed file `index`'s name line: Keep or Undo the file, or Open it. The
/// effect is a redraw, or a notice of why it failed.
#[no_mangle]
pub extern "C" fn nr_changes_file_tool(index: usize, tool: NrChangesTool) -> NrMenuEffect {
    crate::menus::ffi::outcome(use_file_tool(index, tool))
}

/// Runs `tool` of hunk `index`'s strip as last loaded: Keep or Undo the hunk.
#[no_mangle]
pub extern "C" fn nr_changes_hunk_tool(index: usize, tool: NrChangesTool) -> NrMenuEffect {
    crate::menus::ffi::outcome(use_hunk_tool(index, tool))
}

/// Opens file `index` in a tab beside this one, as the drive opens it, so its preview shows.
/// False past the last, or for a file that is gone or outside the drive.
fn nr_changes_open_file(index: usize) -> bool {
    let Some((root, _, row)) = listed(index) else {
        return false;
    };
    let path = root.join(&row.change.path);
    let opened = with_columns(|columns| {
        let id = columns.workspace().id_at(&path)?;
        columns.show_in_tab(&id)
    });
    match opened {
        Some(Ok(())) => {
            crate::ffi::watch_open_columns();
            true
        }
        Some(Err(error)) => {
            eprintln!("the changed file did not open: {error}");
            false
        }
        None => false,
    }
}

// MARK: The picker's menu

/// The picker's entries' ids.
const SCOPE_ENTRIES: [(&str, Scope, char); 3] = [
    ("scope:new", Scope::New, '1'),
    ("scope:uncommitted", Scope::Uncommitted, '2'),
    ("scope:branch", Scope::Branch, '3'),
];
const KEEP_ALL_ENTRY: &str = "keep-all";
const UNDO_ALL_ENTRY: &str = "undo-all";
/// The picker's menu is as wide as this at least.
const SCOPE_MENU_WIDTH: f64 = 200.0;

/// The picker's menu (`NrMenu::ChangesScope`): each scope the root offers, the shown one
/// ticked, with its key; then, below a line, what applies to every file of the scope: Keep all
/// and Undo all in New, nothing in the git scopes.
pub fn menu_changes_scope() -> menus::Menu {
    let language = interface_language();
    let Some(root) = shown_root() else {
        return menus::Menu::new("", Vec::new());
    };
    let (offered, shown, branch, count) = service().read(|engine| {
        let Some(watched) = engine.roots.get(&root) else {
            return (vec![Scope::New], Scope::New, String::new(), 0);
        };
        let branch = view::branch_name(watched).unwrap_or_default().to_owned();
        let count = view::list(engine, &root).len();
        (
            view::offered(watched),
            view::shown_scope(watched),
            branch,
            count,
        )
    });
    let mut entries: Vec<Entry> = SCOPE_ENTRIES
        .iter()
        .filter(|(_, scope, _)| offered.contains(scope))
        .map(|(id, scope, number)| {
            Entry::item(*id, scope_name(*scope, &branch, language))
                .written_keys(Shortcut::command(*number))
                .ticked(*scope == shown)
        })
        .collect();
    if shown == Scope::New {
        entries.push(Entry::separator());
        let say = |words| strings::text(language, words);
        entries.push(Entry::item(KEEP_ALL_ENTRY, say(key::CHANGES_KEEP_ALL)).enabled(count > 0));
        entries.push(Entry::item(UNDO_ALL_ENTRY, say(key::CHANGES_UNDO_ALL)).enabled(count > 0));
    }
    menus::Menu::new("", entries).wide(SCOPE_MENU_WIDTH)
}

/// Runs entry `id` of the picker's menu.
pub(crate) fn run_scope_entry(id: &str) -> Outcome {
    if let Some((_, scope, _)) = SCOPE_ENTRIES.iter().find(|(entry, _, _)| *entry == id) {
        return choose_scope(*scope);
    }
    match id {
        KEEP_ALL_ENTRY => keep_all(),
        UNDO_ALL_ENTRY => undo_all(),
        _ => Ok(false),
    }
}

// MARK: Frames and tools

/// A tool of a name line or a hunk's strip.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrChangesTool {
    /// Opens the file in a tab; only on the name line of a file without diff lines.
    Open,
    /// Takes the change into what the file is diffed from (New only).
    Keep,
    /// Puts the file or hunk back as it is diffed from, its version now in the trash (not vs
    /// main).
    Undo,
}

impl NrChangesTool {
    fn glyph(self) -> Glyph {
        match self {
            Self::Open => Glyph::ArrowSquareOut,
            Self::Keep => Glyph::Check,
            Self::Undo => Glyph::ArrowUUpLeft,
        }
    }

    /// Its tooltip on a name line, for the whole file (`Reach::File`), or in a hunk's strip.
    fn tip(self, reach: Reach) -> strings::Key {
        match (self, reach) {
            (Self::Open, _) => key::CHANGES_OPEN,
            (Self::Keep, Reach::File) => key::CHANGES_KEEP_FILE,
            (Self::Keep, Reach::Hunk) => key::CHANGES_KEEP_HUNK,
            (Self::Undo, Reach::File) => key::CHANGES_UNDO_FILE,
            (Self::Undo, Reach::Hunk) => key::CHANGES_UNDO_HUNK,
        }
    }
}

/// The tools under the pointer in `scope`, on a name line or a hunk: Keep where it keeps, then
/// Undo where it undoes; none in vs main.
fn hover_tools(scope: Scope) -> Vec<NrChangesTool> {
    let keep = scope.keeps().then_some(NrChangesTool::Keep);
    let undo = scope.undoes().then_some(NrChangesTool::Undo);
    keep.into_iter().chain(undo).collect()
}

/// A tool: what it does, where it sits, and its glyph's name and tooltip.
#[repr(C)]
pub struct NrChangesButton {
    pub tool: NrChangesTool,
    pub frame: NrRect,
    pub glyph: NrText,
    pub tip: NrText,
}

/// The most tools a name line has: Keep, Undo and Open.
const MOST_LINE_TOOLS: usize = 3;
/// The most tools a hunk's strip has: Keep and Undo.
const MOST_HUNK_TOOLS: usize = 2;

/// A name line's `count` tools, left to right, and where its path and ending stop
/// (`text_end`, in the tab's points), clear of them.
#[repr(C)]
pub struct NrChangesLineTools {
    pub tools: [NrChangesButton; MOST_LINE_TOOLS],
    pub count: usize,
    pub text_end: f64,
}

fn button(tool: NrChangesTool, reach: Reach, frame: Frame) -> NrChangesButton {
    NrChangesButton {
        tool,
        frame: rect_of(frame),
        glyph: NrText::borrowing(tool.glyph().name()),
        tip: NrText::borrowing(strings::text(interface_language(), tool.tip(reach))),
    }
}

/// `shown` buttons placed at `placed`, in an array of `N`, the rest nowhere.
fn buttons<const N: usize>(
    shown: &[NrChangesTool],
    placed: &[Frame],
    reach: Reach,
) -> [NrChangesButton; N] {
    std::array::from_fn(|at| {
        let tool = shown.get(at).copied().unwrap_or(NrChangesTool::Open);
        button(
            tool,
            reach,
            placed.get(at).copied().unwrap_or(frames::NOWHERE),
        )
    })
}

/// The tools of file `index`'s name line, its top `top` in the tab's points, with the pointer
/// over it (`hovered`) or not; none past the last.
#[no_mangle]
pub extern "C" fn nr_changes_line_tools(
    index: usize,
    top: f64,
    hovered: bool,
) -> NrChangesLineTools {
    let loaded = LOADED.relock();
    let shown = loaded.line_tools(index, hovered);
    let end = frames::tools_end(loaded.laid.diff);
    let placed = frames::line_tools(top, end, shown.len());
    NrChangesLineTools {
        tools: buttons(&shown, &placed, Reach::File),
        count: shown.len(),
        text_end: frames::line_text_end(end, shown.len()),
    }
}

/// A hunk's strip: whether it shows, its frame, and its `count` tools left to right.
#[repr(C)]
pub struct NrChangesStrip {
    pub shown: bool,
    pub frame: NrRect,
    pub tools: [NrChangesButton; MOST_HUNK_TOOLS],
    pub count: usize,
}

/// Hunk `hunk`'s strip, as last loaded, with the diff scrolled to `offset`; not shown past the
/// last, or in vs main, which has no tools.
#[no_mangle]
pub extern "C" fn nr_changes_strip(hunk: usize, offset: f64) -> NrChangesStrip {
    let loaded = LOADED.relock();
    let strip = loaded.strip(hunk, offset);
    let (frame, tools) = strip.unwrap_or((frames::NOWHERE, Vec::new()));
    let (shown, placed): (Vec<NrChangesTool>, Vec<Frame>) = tools.into_iter().unzip();
    NrChangesStrip {
        shown: !shown.is_empty(),
        frame: rect_of(frame),
        tools: buttons(&shown, &placed, Reach::Hunk),
        count: shown.len(),
    }
}

/// The hunk N or P stepped to while the diff is still there, whose strip shows; -1 for none.
#[no_mangle]
pub extern "C" fn nr_changes_current_hunk() -> i64 {
    let stepped = LOADED.relock().stepped_now();
    stepped.map_or(-1, |hunk| hunk as i64)
}

/// What a point is on.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrChangesHitOn {
    Nothing,
    /// `tool` of `file`'s name line.
    Tool,
    /// `file`'s name line, off its tools: a click folds it (`nr_changes_fold`).
    Line,
    /// `tool` of `hunk`'s strip.
    HunkTool,
    /// `hunk`'s rows, off its strip's tools: its strip shows.
    Hunk,
}

/// What a point is on, the file whose line it is and the hunk (-1 for none), the tool, and the
/// frame of what it is on with its tooltip, empty for a line or a hunk.
#[repr(C)]
pub struct NrChangesHit {
    pub on: NrChangesHitOn,
    pub file: i64,
    pub hunk: i64,
    pub tool: NrChangesTool,
    pub frame: NrRect,
    pub tip: NrText,
}

/// What point (`x`, `y`) of the tab is on with the diff scrolled to `offset`: the pinned name
/// line and its tools, then the line under the point (`frames::hit`), then a hunk and its
/// strip. A name line's Keep and Undo count as shown, since the pointer is over the line.
#[no_mangle]
pub extern "C" fn nr_changes_hit(offset: f64, x: f64, y: f64) -> NrChangesHit {
    let loaded = LOADED.relock();
    let tools = |index: usize| loaded.line_tools(index, true);
    let line = frames::hit(&loaded.sections, loaded.laid.diff, offset, (x, y), tools);
    let found = match line {
        Some((frames::Hit::Tool(file, tool), frame)) => Some(Found::LineTool(file, tool, frame)),
        Some((frames::Hit::Line(file), frame)) => Some(Found::Line(file, frame)),
        None => loaded.hunk_hit(offset, (x, y)),
    };
    hit_of(found)
}

fn hit_of(found: Option<Found>) -> NrChangesHit {
    let language = interface_language();
    let nothing = NrChangesHit {
        on: NrChangesHitOn::Nothing,
        file: -1,
        hunk: -1,
        tool: NrChangesTool::Open,
        frame: rect_of(frames::NOWHERE),
        tip: NrText::borrowing(""),
    };
    let tip =
        |tool: NrChangesTool, reach| NrText::borrowing(strings::text(language, tool.tip(reach)));
    match found {
        None => nothing,
        Some(Found::LineTool(file, tool, frame)) => NrChangesHit {
            on: NrChangesHitOn::Tool,
            file: file as i64,
            tool,
            frame: rect_of(frame),
            tip: tip(tool, Reach::File),
            ..nothing
        },
        Some(Found::Line(file, frame)) => NrChangesHit {
            on: NrChangesHitOn::Line,
            file: file as i64,
            frame: rect_of(frame),
            ..nothing
        },
        Some(Found::HunkTool(hunk, tool, frame)) => NrChangesHit {
            on: NrChangesHitOn::HunkTool,
            hunk: hunk as i64,
            tool,
            frame: rect_of(frame),
            tip: tip(tool, Reach::Hunk),
            ..nothing
        },
        Some(Found::Hunk(hunk)) => NrChangesHit {
            on: NrChangesHitOn::Hunk,
            hunk: hunk as i64,
            ..nothing
        },
    }
}

/// Where the tab's parts go in its frame, in the tab's points (`changes::frames`): the picker
/// on the list's first line, the file list under it and the diff; the diff's scrolling
/// `extent`; `relaid` when its rows now wrap differently, so the shell loads again; the height
/// of a list row, a diff line and a file's name line; and the inset and gap between controls.
#[repr(C)]
pub struct NrChangesFrames {
    pub relaid: bool,
    pub picker: NrRect,
    pub list: NrRect,
    pub diff: NrRect,
    pub extent: f64,
    pub list_row: f64,
    pub diff_line: f64,
    pub separator: f64,
    pub inset: f64,
    pub gap: f64,
}

fn rect_of(frame: Frame) -> NrRect {
    NrRect {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
    }
}

/// The tab's parts in a frame `width` × `height`, its diff's rows wrapped to fit in a monospace
/// face whose figures are `advance` wide. A new width or face wraps them again, keeping the file
/// in view where it was.
#[no_mangle]
pub extern "C" fn nr_changes_frames(width: f64, height: f64, advance: f64) -> NrChangesFrames {
    let laid = frames::frames(width, height);
    let mut loaded = LOADED.relock();
    let relaid = loaded.laid.diff.width != laid.diff.width || loaded.advance != advance;
    loaded.laid = laid;
    loaded.advance = advance;
    if relaid {
        relay(&mut loaded);
    }
    NrChangesFrames {
        relaid,
        picker: rect_of(laid.picker),
        list: rect_of(laid.list),
        diff: rect_of(laid.diff),
        extent: loaded.sections.extent(laid.diff.height),
        list_row: frames::LIST_ROW,
        diff_line: frames::DIFF_LINE,
        separator: frames::SEPARATOR,
        inset: frames::INSET,
        gap: frames::GAP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::snapshot::{Content, Entry};
    use crate::changes::store::Hash;
    use crate::changes::view::FileChange;

    #[test]
    fn the_review_button_says_how_many_files_wait() {
        assert_eq!(review_words(4, "en"), "Review changes · 4 files");
        assert_eq!(review_words(1, "en"), "Review changes · 1 file");
        assert_eq!(review_words(0, "en"), "Review changes");
    }

    #[test]
    fn a_diff_s_name_and_extension_are_its_file_s() {
        assert_eq!(file_extension("src/main.rs"), "rs");
        assert_eq!(file_extension("a.d/notes.md"), "md");
        assert_eq!(file_name("Research/shot.png"), "shot.png");
    }

    /// A listed file of `status` with `rows` diff rows, its hunk headers at `headers`.
    fn section(path: &str, status: Status, rows: usize, headers: &[usize]) -> Section {
        let entry = || {
            Some(Entry {
                stamp: crate::changes::scan::Stamp {
                    bytes: 1,
                    modified_ns: 0,
                },
                content: Content::Binary(Hash::of(path.as_bytes())),
            })
        };
        let (before, after) = match status {
            Status::Added => (None, entry()),
            Status::Modified => (entry(), entry()),
            Status::Deleted => (entry(), None),
        };
        let rows: Vec<diff::Row> = (0..rows)
            .map(|at| diff::Row {
                hunk: 0,
                cell: match headers.contains(&at) {
                    true => Cell::Header,
                    false => Cell::Line(at),
                },
            })
            .collect();
        let lines = rows.len();
        Section {
            key: 0,
            row: FileRow {
                change: FileChange {
                    path: path.to_owned(),
                    before,
                    after,
                },
                status,
                counts: None,
                contested: false,
                entered_ms: 0,
            },
            shown: view::Shown::default(),
            rows,
            headers: Vec::new(),
            line_words: Vec::new(),
            words: Vec::new(),
            name: file_name(path).to_owned(),
            extension: String::new(),
            noisy: false,
            folded: false,
            gutter: 0.0,
            row_breaks: Vec::new(),
            breaks: Vec::new(),
            row_lines: Vec::new(),
            lines,
            wrapped_at: None,
        }
    }

    /// A text file's section with its real diff from `old` to `new`.
    fn text_section(old: &str, new: &str) -> Section {
        let clock = crate::changes::testing::FixedClock::at(0);
        let deadline = diff::Deadline {
            clock: &clock,
            until_ms: u64::MAX,
        };
        let mut file = section("notes.md", Status::Modified, 0, &[]);
        file.shown.diff = diff::diff(old, new, &deadline);
        file.rows = diff::rows(&file.shown.diff);
        file.headers = file
            .shown
            .diff
            .hunks
            .iter()
            .map(diff::Hunk::header)
            .collect();
        (file.line_words, file.words) = flatten_words(&file.shown.diff);
        file.lines = file.rows.len();
        file
    }

    #[test]
    fn a_long_line_wraps_to_the_diff_s_width_and_pushes_what_follows_down() {
        let long = "word ".repeat(60);
        let old = "a\nb\n".to_owned();
        let new = format!("a\n{long}\nb\n");
        let mut file = text_section(&old, &new);
        // Rows: the header, a, the long line, b.
        assert_eq!(file.rows.len(), 4);
        let advance = 7.0;
        let width = 600.0;
        file.wrap(width, advance);
        let columns = frames::text_columns(width, file.gutter, advance);
        assert_eq!(file.gutter, frames::gutter(2, advance));
        let wraps = diff::wrap(&long, 8, columns).len();
        assert!(wraps > 1, "the long line wraps");
        assert_eq!(file.lines, 3 + wraps);
        let long_row = row_of(&file, 2, file.rows[2]);
        assert_eq!((long_row.line, long_row.break_count), (2, wraps));
        assert_eq!(file.breaks[long_row.break_first], 0);
        // The row after it starts below all its lines, and the section is that much taller.
        assert_eq!(row_of(&file, 3, file.rows[3]).line, 2 + wraps);
        assert_eq!(file.height(), frames::section_height(3 + wraps, false));
        // Wrapped once per width: the same width leaves it, another wraps it again.
        file.wrap(width, advance);
        assert_eq!(file.lines, 3 + wraps);
        file.wrap(3000.0, advance);
        assert_eq!(file.lines, 4);
        // Unmeasured, nothing wraps.
        file.wrap(width, 0.0);
        assert_eq!(file.lines, 4);
    }

    #[test]
    fn a_wrapped_row_moves_the_hunks_after_it_down() {
        let long = "x".repeat(400);
        let old = (1..=20).map(|n| format!("{n}\n")).collect::<String>();
        let new = old
            .replacen("2\n", &format!("2{long}\n"), 1)
            .replacen("19\n", "19!\n", 1);
        let mut file = text_section(&old, &new);
        assert_eq!(file.shown.diff.hunks.len(), 2);
        let tops = |file: &Section| -> Vec<f64> {
            let sections = Sections::new([file.height()]);
            hunk_places(std::slice::from_ref(file), &sections)
                .iter()
                .map(|place| place.top)
                .collect()
        };
        let unwrapped = tops(&file);
        file.wrap(600.0, 7.0);
        let extra = (file.lines - file.rows.len()) as f64 * frames::DIFF_LINE;
        assert!(extra > 0.0);
        let wrapped = tops(&file);
        assert_eq!(wrapped[0], unwrapped[0]);
        assert_eq!(wrapped[1], unwrapped[1] + extra);
    }

    /// A Loaded of `files`, laid out as `view` folds them.
    fn loaded(files: Vec<Section>, view: &ViewState) -> Loaded {
        loaded_in(Scope::New, files, view)
    }

    /// A Loaded of `files` in `scope`, laid out as `view` folds them.
    fn loaded_in(scope: Scope, files: Vec<Section>, view: &ViewState) -> Loaded {
        let mut loaded = Loaded {
            root: None,
            scope,
            picker: String::new(),
            empty: String::new(),
            stepped: None,
            files,
            sections: Sections::empty(),
            hunks: Vec::new(),
            in_view: None,
            offset: 0.0,
            keys: 0,
            laid: frames::frames(1200.0, 800.0),
            advance: 0.0,
        };
        loaded.lay(Some(view));
        loaded
    }

    #[test]
    fn a_collapsed_file_is_its_name_line_and_its_hunks_are_passed_over() {
        let mut picture = section("shot.png", Status::Added, 0, &[]);
        picture.noisy = true;
        let files = vec![
            section("a.txt", Status::Modified, 10, &[0, 6]),
            picture,
            section("b.txt", Status::Modified, 4, &[0]),
        ];
        let mut view = ViewState::default();
        let open = loaded(files, &view);
        // The noisy picture starts collapsed, the rest expanded.
        let folded: Vec<bool> = open.files.iter().map(|file| file.folded).collect();
        assert_eq!(folded, [false, true, false]);
        assert_eq!(open.sections.height(1), Some(frames::SEPARATOR));
        assert_eq!(open.hunks.len(), 3);
        // Collapsing the first: its hunks are passed over, the rest move up.
        view.folds.insert("a.txt".into(), true);
        let files = open.files;
        let shut = loaded(files, &view);
        assert_eq!(shut.sections.top(2), Some(2.0 * frames::SEPARATOR));
        let tops: Vec<f64> = shut.hunks.iter().map(|place| place.top).collect();
        assert_eq!(tops, [2.0 * frames::SEPARATOR + frames::SEPARATOR]);
        // Expanding the picture shows its block.
        view.folds.insert("shot.png".into(), false);
        let shown = loaded(shut.files, &view);
        assert_eq!(
            shown.sections.height(1),
            Some(frames::section_height(0, false))
        );
    }

    #[test]
    fn a_name_line_has_keep_and_undo_on_hover_and_open_for_media() {
        let lined = section("a.txt", Status::Modified, 3, &[0]);
        let picture = section("shot.png", Status::Added, 0, &[]);
        let gone = section("old.png", Status::Deleted, 0, &[]);
        use NrChangesTool::{Keep, Open, Undo};
        assert!(lined.line_tools(false, Scope::New).is_empty());
        assert_eq!(lined.line_tools(true, Scope::New), [Keep, Undo]);
        assert_eq!(picture.line_tools(false, Scope::New), [Open]);
        assert_eq!(picture.line_tools(true, Scope::New), [Keep, Undo, Open]);
        assert_eq!(gone.line_tools(true, Scope::New), [Keep, Undo]);
        // Uncommitted only undoes; vs main is read only.
        assert_eq!(lined.line_tools(true, Scope::Uncommitted), [Undo]);
        assert!(lined.line_tools(true, Scope::Branch).is_empty());
        assert_eq!(picture.line_tools(true, Scope::Branch), [Open]);
        let tips = |reach| -> Vec<&str> {
            [Keep, Undo, Open]
                .map(|tool| strings::text("en", tool.tip(reach)))
                .into()
        };
        assert_eq!(
            tips(Reach::File),
            ["Keep file (⌘⇧Y)", "Undo file (⌘⇧⌥Z)", "Open"]
        );
        assert_eq!(tips(Reach::Hunk)[..2], ["Keep (⌘Y)", "Undo (⌘⌥Z)"]);
        assert_eq!(picture.line_tools(true, Scope::New).len(), MOST_LINE_TOOLS);
    }

    #[test]
    fn a_click_on_a_line_s_icon_is_its_tool_and_elsewhere_folds_the_line() {
        // The picture starts collapsed, as media does.
        let mut picture = section("shot.png", Status::Added, 0, &[]);
        picture.noisy = true;
        let files = vec![picture, section("a.txt", Status::Modified, 30, &[0])];
        let laid = loaded(files, &ViewState::default());
        let end = frames::tools_end(laid.laid.diff);
        let at = |offset: f64, x: f64, y: f64| {
            frames::hit(&laid.sections, laid.laid.diff, offset, (x, y), |index| {
                laid.line_tools(index, true)
            })
            .map(|(hit, _)| hit)
        };
        let icon = |slot: f64| end - (slot + 0.5) * frames::LINE_TOOL;
        use frames::Hit;
        use NrChangesTool::{Keep, Open, Undo};
        // The collapsed picture: Open, Undo, Keep from the right, then the line.
        assert_eq!(at(0.0, icon(0.0), 16.0), Some(Hit::Tool(0, Open)));
        assert_eq!(at(0.0, icon(1.0), 16.0), Some(Hit::Tool(0, Undo)));
        assert_eq!(at(0.0, icon(2.0), 16.0), Some(Hit::Tool(0, Keep)));
        assert_eq!(at(0.0, icon(3.0), 16.0), Some(Hit::Line(0)));
        // The text file's line below it has Undo and Keep.
        let below = frames::SEPARATOR + 16.0;
        assert_eq!(at(0.0, icon(0.0), below), Some(Hit::Tool(1, Undo)));
        assert_eq!(at(0.0, icon(1.0), below), Some(Hit::Tool(1, Keep)));
        assert_eq!(at(0.0, icon(2.0), below), Some(Hit::Line(1)));
        // Scrolled into the text file, its pinned line's tools work at the top.
        let into = frames::SEPARATOR + 100.0;
        assert_eq!(at(into, icon(0.0), 10.0), Some(Hit::Tool(1, Undo)));
    }

    /// Two text files of 30 rows, a hunk header at rows 0 and 15 of the first and 0 of the
    /// second, in `scope`.
    fn two_hunked(scope: Scope) -> Loaded {
        let files = vec![
            section("a.txt", Status::Modified, 30, &[0, 15]),
            section("b.txt", Status::Modified, 30, &[0]),
        ];
        loaded_in(scope, files, &ViewState::default())
    }

    #[test]
    fn a_hunk_under_the_pointer_shows_keep_and_undo_at_its_first_row_s_right_end() {
        let laid = two_hunked(Scope::New);
        assert_eq!(laid.hunks.len(), 3);
        let first = laid.hunks[0];
        assert_eq!((first.file, first.top), (0, frames::SEPARATOR));
        assert_eq!(first.bottom, frames::SEPARATOR + 15.0 * frames::DIFF_LINE);
        assert_eq!(laid.hunks[1].top, first.bottom);
        // Its strip sits on its header row, at the diff's right end: Keep, then Undo.
        let (strip, tools) = laid.strip(0, 0.0).expect("a strip");
        let diff = laid.laid.diff;
        assert_eq!(strip.y, diff.y + first.top);
        assert_eq!(strip.x + strip.width, frames::tools_end(diff));
        let kinds: Vec<NrChangesTool> = tools.iter().map(|(tool, _)| *tool).collect();
        assert_eq!(kinds, [NrChangesTool::Keep, NrChangesTool::Undo]);
        // The pointer on Undo is that tool of hunk 0; elsewhere on the hunk, the hunk.
        let undo = tools[1].1;
        let middle = (undo.x + undo.width / 2.0, undo.y + undo.height / 2.0);
        assert_eq!(
            laid.hunk_hit(0.0, middle),
            Some(Found::HunkTool(0, NrChangesTool::Undo, undo))
        );
        let row = diff.y + first.top + 5.0 * frames::DIFF_LINE;
        assert_eq!(
            laid.hunk_hit(0.0, (diff.x + 40.0, row)),
            Some(Found::Hunk(0))
        );
        let second = diff.y + laid.hunks[1].top + 1.0;
        assert_eq!(
            laid.hunk_hit(0.0, (diff.x + 40.0, second)),
            Some(Found::Hunk(1))
        );
        // Scrolled into the hunk, its strip stays just under the pinned name line.
        let offset = first.top + 4.0 * frames::DIFF_LINE;
        let (strip, _) = laid.strip(0, offset).expect("a strip");
        assert_eq!(strip.y, diff.y + frames::SEPARATOR);
        // Uncommitted has Undo alone.
        let uncommitted = two_hunked(Scope::Uncommitted);
        let (_, tools) = uncommitted.strip(0, 0.0).expect("a strip");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].0, NrChangesTool::Undo);
    }

    #[test]
    fn vs_main_is_read_only_with_no_keep_or_undo_to_hit() {
        let laid = two_hunked(Scope::Branch);
        assert_eq!(laid.strip(0, 0.0), None);
        let diff = laid.laid.diff;
        // Where Keep and Undo would be, on a hunk's first row: only the hunk.
        let (_, tools) = two_hunked(Scope::New).strip(0, 0.0).expect("a strip");
        for (_, frame) in tools {
            let middle = (frame.x + frame.width / 2.0, frame.y + frame.height / 2.0);
            assert_eq!(laid.hunk_hit(0.0, middle), Some(Found::Hunk(0)));
        }
        // A name line has no tools either: the whole line folds.
        let end = frames::tools_end(diff);
        for slot in 0..3 {
            let x = end - (f64::from(slot) + 0.5) * frames::LINE_TOOL;
            let found = frames::hit(&laid.sections, diff, 0.0, (x, 16.0), |index| {
                laid.line_tools(index, true)
            });
            assert_eq!(found.map(|(hit, _)| hit), Some(frames::Hit::Line(0)));
        }
        assert_eq!(hover_tools(Scope::Branch), []);
    }

    #[test]
    fn the_current_hunk_is_the_one_stepped_to_else_the_first_shown_below_the_pinned_line() {
        let mut laid = two_hunked(Scope::New);
        assert_eq!(laid.current_hunk(), Some(0));
        // Scrolled past most of the first hunk: still it, while a row shows under the line.
        laid.offset = laid.hunks[0].bottom - frames::SEPARATOR - 1.0;
        assert_eq!(laid.current_hunk(), Some(0));
        laid.offset = laid.hunks[0].bottom - frames::SEPARATOR;
        assert_eq!(laid.current_hunk(), Some(1));
        // N steps to the next header, which is current while the diff stays there.
        laid.offset = 0.0;
        let stepped = stepped_hunk(&laid.hunks, laid.offset, 1).expect("next");
        assert_eq!(stepped, (0, laid.hunks[0].top));
        let stepped = stepped_hunk(&laid.hunks, laid.hunks[0].top, 1).expect("next");
        assert_eq!(stepped, (1, laid.hunks[1].top));
        laid.stepped = Some(stepped);
        laid.offset = stepped.1;
        assert_eq!(laid.stepped_now(), Some(1));
        assert_eq!(laid.current_hunk(), Some(1));
        laid.offset += 40.0;
        assert_eq!(laid.stepped_now(), None);
    }

    #[test]
    fn the_picker_says_the_scope_and_its_count_and_each_scope_its_empty_words() {
        use crate::changes::git::{DefaultBranch, Repository};
        let mut engine = Engine::default();
        let root = Path::new("/p");
        engine.watch_root(root, || None);
        let words = |engine: &Engine, count| {
            let (_, picker, empty) = scope_words(engine, root, count);
            (picker, empty)
        };
        assert_eq!(
            words(&engine, 4),
            (
                "New · 4".to_owned(),
                "Nothing new from your agents.".to_owned()
            )
        );
        let main = DefaultBranch {
            name: "main".into(),
            reference: "refs/heads/main".into(),
        };
        let repository = Repository {
            uncommitted: Default::default(),
            branch: Some((main, Default::default())),
        };
        engine.git_read(root, Some(repository));
        assert!(view::choose(&mut engine, root, Scope::Uncommitted));
        assert_eq!(
            words(&engine, 0),
            (
                "Uncommitted · 0".to_owned(),
                "No uncommitted changes.".to_owned()
            )
        );
        assert!(view::choose(&mut engine, root, Scope::Branch));
        assert_eq!(
            words(&engine, 2),
            ("vs main · 2".to_owned(), "No changes vs main.".to_owned())
        );
    }

    #[test]
    fn hunks_step_from_the_diff_s_top_edge_across_files() {
        let files = [
            section("a.txt", Status::Modified, 10, &[0, 6]),
            section("b.txt", Status::Modified, 4, &[0]),
        ];
        let sections = Sections::new(files.iter().map(Section::height));
        let hunks = hunk_places(&files, &sections);
        let tops: Vec<f64> = hunks.iter().map(|place| place.top).collect();
        let second = frames::SEPARATOR + 6.0 * frames::DIFF_LINE;
        let third = sections.top(1).expect("b") + frames::SEPARATOR;
        assert_eq!(tops, [frames::SEPARATOR, second, third]);
        let at = |offset, step| stepped_hunk(&hunks, offset, step).map(|(_, top)| top);
        assert_eq!(at(0.0, 1), Some(frames::SEPARATOR));
        assert_eq!(at(frames::SEPARATOR, 1), Some(second));
        assert_eq!(at(second, 1), Some(third));
        assert_eq!(at(third, 1), None);
        assert_eq!(at(third, -1), Some(second));
        assert_eq!(at(0.0, -1), None);
        // A hunk runs to the next one's header, the last to its file's last row.
        assert_eq!(hunks[0].bottom, second);
        assert_eq!(
            hunks[1].bottom,
            frames::SEPARATOR + 10.0 * frames::DIFF_LINE
        );
    }

    #[test]
    fn keys_step_files_and_hunks_and_fold_and_r_does_nothing() {
        let press = |key: NrKey, character: char| NrKeyPress {
            key,
            character: character as u32,
            letter: character as u32,
            held: 0,
        };
        assert_eq!(asked(&press(NrKey::Other, 'j')), Some(Asked::File(1)));
        assert_eq!(asked(&press(NrKey::Down, '\0')), Some(Asked::File(1)));
        assert_eq!(asked(&press(NrKey::Other, 'K')), Some(Asked::File(-1)));
        assert_eq!(asked(&press(NrKey::Up, '\0')), Some(Asked::File(-1)));
        assert_eq!(asked(&press(NrKey::Other, 'n')), Some(Asked::Hunk(1)));
        assert_eq!(asked(&press(NrKey::Other, 'p')), Some(Asked::Hunk(-1)));
        assert_eq!(asked(&press(NrKey::Other, 'r')), None);
        assert_eq!(asked(&press(NrKey::Other, 'q')), None);
        let held = |held, key| NrKeyPress {
            held,
            ..press(NrKey::Other, key)
        };
        use crate::ffi::{NR_HELD_COMMAND, NR_HELD_CONTROL, NR_HELD_OPTION, NR_HELD_SHIFT};
        assert_eq!(asked(&held(NR_HELD_COMMAND, 'j')), None);
        assert_eq!(asked(&held(NR_HELD_OPTION, 'j')), None);
        // X folds the file in view, ⌥X every file; ⌘X stays Cut.
        assert_eq!(asked(&press(NrKey::Other, 'x')), Some(Asked::Fold(false)));
        assert_eq!(asked(&held(NR_HELD_OPTION, 'x')), Some(Asked::Fold(true)));
        assert_eq!(asked(&held(NR_HELD_COMMAND, 'x')), None);
        // ⌘1 to ⌘3 choose the scope.
        let command = |key| held(NR_HELD_COMMAND, key);
        assert_eq!(asked(&command('1')), Some(Asked::Scope(Scope::New)));
        assert_eq!(asked(&command('2')), Some(Asked::Scope(Scope::Uncommitted)));
        assert_eq!(asked(&command('3')), Some(Asked::Scope(Scope::Branch)));
        assert_eq!(asked(&command('4')), None);
        // ⌘Y keeps and ⌘⌥Z undoes the current hunk; with ⇧, the file in view.
        assert_eq!(asked(&command('y')), Some(Asked::Keep(Reach::Hunk)));
        let shift = NR_HELD_COMMAND | NR_HELD_SHIFT;
        assert_eq!(asked(&held(shift, 'y')), Some(Asked::Keep(Reach::File)));
        let option = NR_HELD_COMMAND | NR_HELD_OPTION;
        assert_eq!(asked(&held(option, 'z')), Some(Asked::Undo(Reach::Hunk)));
        let both = option | NR_HELD_SHIFT;
        assert_eq!(asked(&held(both, 'z')), Some(Asked::Undo(Reach::File)));
        // ⌘Z puts the last Undo back; ⌘⇧Z and ⌃ keys pass on.
        assert_eq!(asked(&command('z')), Some(Asked::Restore));
        assert_eq!(asked(&held(shift, 'z')), None);
        assert_eq!(asked(&held(NR_HELD_COMMAND | NR_HELD_CONTROL, 'y')), None);
    }

    #[test]
    fn the_picker_s_menu_ids_name_each_scope_once() {
        let ids: Vec<&str> = SCOPE_ENTRIES.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(ids, ["scope:new", "scope:uncommitted", "scope:branch"]);
        assert!(!ids.contains(&KEEP_ALL_ENTRY) && !ids.contains(&UNDO_ALL_ENTRY));
    }
}
