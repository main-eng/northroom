//! A Changes tab (CONTEXT.md): what the terminals changed in a root since the person last kept
//! it, in the folder's place while its tab is active. The core lists, diffs and places
//! everything (`changes::ffi`, `nr_changes_frames`); this screen draws what it reads and passes
//! input on.
//!
//! On the left, every listed file in one flat list from the top: the path, then `+3 −1` for text or "new",
//! "changed" or "deleted", with a warning when two terminals changed it at once. On the right,
//! one continuous diff of them all, a section per file that starts with a slim muted line naming
//! it behind a chevron: monospace 12 px on 16 px lines as source files show, line numbers, added
//! and removed lines on the status colours at low opacity, changed words stronger. A line too
//! long for the diff wraps where the core says (`diff::wrap`), its numbers on its first line
//! only, as GitHub's and GitLab's reviews do, so nothing scrolls sideways; the list and the diff
//! each scroll up and down on their own. A file without diff lines (a picture, a video, any
//! other binary) shows a small block with its name and what happened.
//!
//! A click on a name line collapses its file to that line, which then ends in the row's
//! ending, muted; ⌥-click folds every file the same way. A click in the list expands the file
//! it scrolls to. The core decides what folds and keeps the file in view where it was.
//!
//! Keep and Undo show under the pointer, at the far right, as Zed's review shows them: on a name
//! line for the whole file (`nr_changes_line_tools`, beside Open for a file without diff lines),
//! and on a hunk in a small strip at its first row's right end (`nr_changes_strip`), which also
//! shows on the hunk N and P stepped to. They are quiet icon buttons with tooltips naming their
//! keys. A click on a tool does only what it does; the rest of the line folds.
//!
//! The file whose section holds the diff's top edge is in view (the core's scroll-spy,
//! `nr_changes_scrolled`): it lights in the list, which scrolls to keep it shown, and its name
//! line stays pinned at the diff's top over its rows, on the background with a hairline under
//! it, until the next file's line pushes it up (`nr_changes_pinned`). A click in the list
//! scrolls the diff to that file. With nothing listed, one plain line in the diff's place says
//! so.
//!
//! Keys go to the core (`nr_changes_key`): J and K or Down and Up for files, N and P for hunks,
//! X to fold the file in view (⌥X every file), ⌘Y and ⌘⌥Z to keep and undo the current hunk (with ⇧ its file), and ⌘Z right after an Undo to put it back.
//!
//! Only the sections on screen are drawn, and a file's rows are read from the core the first
//! time it shows. On a Mac their lines take syntax colours from the core's highlighter
//! (`nr_syntax_call` on each side's whole text, `webhost::syntax`); they stay plain until it
//! answers, and changed words keep their stronger ground under the coloured text.

use super::source::expand_tabs;
use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use crate::script::{Answer, Pointer, Route, Step, Table, workspace};
use gpui::{
    App, Bounds, ContentMask, Context, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollEvent, Styled, Task, Window,
    canvas, point, px,
};
use kit::core::{c, contains, frame, inset, offset, rect, text, zeroed};
use kit::draw::{fill, rounded, stroke};
use kit::icons::{self, Glyph};
use kit::lists::rename::Said;
use kit::lists::scroll::Scroller;
use kit::motion::now;
use kit::nr::*;
use kit::text::{draw_line, width};
use kit::theme::{Face, Token, color, faded, mixed};
use std::collections::HashMap;

/// Monospace 12 px, as source files show; lines are the core's `diff_line` tall.
fn mono() -> Face {
    Face::mono(12.0)
}

fn path_face() -> Face {
    Face::sans(12.0, 400.0)
}

/// Added and removed lines sit on their status colour at this opacity; changed words at the
/// stronger one.
const LINE_GROUND: f32 = 0.12;
const WORD_GROUND: f32 = 0.32;
/// Overlays for hover and press (DECISIONS.md, design rules).
const HOVER: f32 = 0.08;
const PRESS: f32 = 0.12;
/// Corners of a block (a surface) and of a tool (a control), and a hunk strip's.
const BLOCK_CORNER: f32 = 8.0;
const TOOL_CORNER: f32 = 6.0;
const STRIP_CORNER: f32 = 4.0;
const GLYPH: f32 = 16.0;
/// A hunk strip's glyphs, smaller to fit a diff line.
const STRIP_GLYPH: f32 = 13.0;
/// Every fixed word the tab draws besides its tools' tooltips (`nr_changes_words`).
#[derive(Default)]
struct Words {
    /// What the tab says with nothing listed.
    empty: String,
    whole: String,
    new: String,
    changed: String,
    deleted: String,
    file_added: String,
    file_modified: String,
    file_deleted: String,
}

impl Words {
    fn read() -> Self {
        let words = nr_changes_words();
        Self {
            empty: text(&words.empty),
            whole: text(&words.whole),
            new: text(&words.new),
            changed: text(&words.changed),
            deleted: text(&words.deleted),
            file_added: text(&words.file_added),
            file_modified: text(&words.file_modified),
            file_deleted: text(&words.file_deleted),
        }
    }

    /// A row's ending for a file of `status` without line counts.
    fn status_word(&self, status: NrChangesStatus) -> &str {
        match status {
            NrChangesStatus::Added => &self.new,
            NrChangesStatus::Modified => &self.changed,
            NrChangesStatus::Deleted => &self.deleted,
        }
    }

    /// What happened, in the block of a file without diff lines.
    fn happened(&self, status: NrChangesStatus) -> &str {
        match status {
            NrChangesStatus::Added => &self.file_added,
            NrChangesStatus::Modified => &self.file_modified,
            NrChangesStatus::Deleted => &self.file_deleted,
        }
    }
}

// MARK: What the core lists

/// How a file row ends.
#[derive(Clone, Debug, PartialEq)]
enum Ending {
    /// Lines added and removed.
    Counts(u32, u32),
    /// new, changed or deleted.
    Word(String),
    /// A text file not counted yet.
    Nothing,
}

/// One listed file as last loaded: its row and where its section sits down the diff.
struct File {
    /// The same while its diff is, so its rows and colours are kept.
    key: u64,
    path: String,
    name: String,
    status: NrChangesStatus,
    ending: Ending,
    contested: bool,
    /// Too different to compare line by line.
    whole: bool,
    /// Collapsed to its name line.
    folded: bool,
    rows: usize,
    /// One line-number column's width, from the core.
    gutter: f32,
    top: f32,
    height: f32,
}

impl File {
    /// File `index` as last loaded.
    fn read(index: usize, words: &Words) -> Option<Self> {
        let mut file: NrChangesFile = zeroed();
        // SAFETY: storage for one file.
        if !unsafe { nr_changes_file(index, &mut file) } {
            return None;
        }
        let lined = file.text && file.status == NrChangesStatus::Modified;
        let ending = match (lined, file.counted) {
            (true, false) => Ending::Nothing,
            (true, true) => Ending::Counts(file.plus, file.minus),
            (false, _) => Ending::Word(words.status_word(file.status).to_owned()),
        };
        Some(Self {
            key: file.key,
            path: text(&file.path),
            name: text(&file.name),
            status: file.status,
            ending,
            contested: file.contested,
            whole: file.whole,
            folded: file.folded,
            rows: file.rows,
            gutter: file.gutter as f32,
            top: file.top as f32,
            height: file.height as f32,
        })
    }
}

/// One drawn row of a diff: what it shows, its line numbers (0 for none), its text with tabs
/// expanded, and its changed words as x spans from the text's start. It starts on visual line
/// `line` of its file and wraps where the core says: `wraps` holds the x, from the text's
/// start, where each of its visual lines begins (the first 0).
struct Row {
    shows: NrChangesRowShows,
    old: u32,
    new: u32,
    text: String,
    words: Vec<(f32, f32)>,
    line: usize,
    wraps: Vec<f32>,
}

impl Row {
    fn read(file: usize, row: &NrChangesRow, tab: usize, advance: f32) -> Self {
        let raw = text(&row.text);
        let words = (row.word_first..row.word_first + row.word_count)
            .filter_map(|word| word_span(&raw, nr_changes_word(file, word), tab, advance))
            .collect();
        let text = expand_tabs(&raw, 0, tab);
        let wraps = (row.break_first..row.break_first + row.break_count)
            .map(|index| {
                let at = nr_changes_break(file, index);
                let byte = text
                    .char_indices()
                    .nth(at)
                    .map_or(text.len(), |(byte, _)| byte);
                x_of(&text[..byte], advance)
            })
            .collect::<Vec<f32>>();
        Self {
            shows: row.shows,
            old: row.old_line,
            new: row.new_line,
            text,
            words,
            line: row.line,
            wraps: if wraps.is_empty() { vec![0.0] } else { wraps },
        }
    }

    /// How many visual lines it takes.
    fn height(&self) -> usize {
        self.wraps.len()
    }

    /// The ground under the whole line, if any.
    fn ground(&self) -> Option<Hsla> {
        match self.shows {
            NrChangesRowShows::Added => Some(mixed(Token::StatusSuccess, LINE_GROUND)),
            NrChangesRowShows::Removed => Some(mixed(Token::Destructive, LINE_GROUND)),
            NrChangesRowShows::Header => Some(color(Token::Muted)),
            NrChangesRowShows::Context => None,
        }
    }

    /// The side (0 before, 1 after) and line number whose syntax colours it takes.
    fn coloured_line(&self) -> Option<(usize, u32)> {
        match self.shows {
            NrChangesRowShows::Removed => Some((0, self.old)),
            NrChangesRowShows::Added | NrChangesRowShows::Context => Some((1, self.new)),
            NrChangesRowShows::Header => None,
        }
    }

    /// The ground under its changed words.
    fn word_ground(&self) -> Hsla {
        match self.shows {
            NrChangesRowShows::Removed => mixed(Token::Destructive, WORD_GROUND),
            _ => mixed(Token::StatusSuccess, WORD_GROUND),
        }
    }
}

/// Where text after `before` (tabs expanded) starts, in a monospace face whose figures are
/// `advance` wide: by its cells, as the core wraps (`diff::cells`), rather than shaping it.
fn x_of(before: &str, advance: f32) -> f32 {
    northroom_core::changes::diff::cells(before) as f32 * advance
}

/// The x span of byte range `range` of `raw`, tabs expanded, in the monospace face.
fn word_span(raw: &str, range: NrChangesRange, tab: usize, advance: f32) -> Option<(f32, f32)> {
    let x = |at: usize| {
        raw.get(..at)
            .map(|before| x_of(&expand_tabs(before, 0, tab), advance))
    };
    Some((x(range.start)?, x(range.end)?))
}

/// A file's drawn rows, read the first time it shows, and how wide the monospace figures they
/// were placed by are.
struct Lines {
    rows: Vec<Row>,
    advance: f32,
}

impl Lines {
    /// File `file`'s `count` rows as last loaded.
    fn read(file: usize, count: usize) -> Self {
        let tab = nr_source_tab_spaces() as usize;
        let advance = width("0", mono());
        let rows: Vec<Row> = (0..count)
            .filter_map(|index| {
                let mut row: NrChangesRow = zeroed();
                // SAFETY: storage for one row.
                unsafe { nr_changes_diff_row(file, index, &mut row) }
                    .then(|| Row::read(file, &row, tab, advance))
            })
            .collect();
        Self { rows, advance }
    }
}

// MARK: Syntax colours

/// A stretch of a coloured line: its text, its x from the line's start, and its colour (`None`
/// for the foreground).
struct Stroke {
    text: String,
    x: f32,
    ink: Option<Hsla>,
}

/// One line of a side as the highlighter coloured it, its text with tabs expanded.
#[derive(Default)]
struct ColouredLine {
    text: String,
    strokes: Vec<Stroke>,
}

/// The syntax colours of a file's two sides, by line from 1, once answered.
#[derive(Default)]
struct Colours {
    sides: [Vec<ColouredLine>; 2],
}

impl Colours {
    /// The coloured line `row` shows, when its text is the one coloured.
    fn line(&self, row: &Row) -> Option<&ColouredLine> {
        let (side, number) = row.coloured_line()?;
        let index = usize::try_from(number).ok()?.checked_sub(1)?;
        self.sides[side]
            .get(index)
            .filter(|line| line.text == row.text)
    }
}

/// The highlighter's expression for `code` from a file with `extension`; empty when it stays
/// plain.
fn syntax_call(extension: &str, code: &str) -> String {
    let (extension, code) = (c(extension), c(code));
    // SAFETY: NUL-terminated texts.
    text(&unsafe { nr_syntax_call(extension.as_ptr(), code.as_ptr()) })
}

/// The coloured lines of the highlighter's `answer`, tabs expanded every `tab` columns.
fn coloured_lines(answer: &str, tab: usize) -> Vec<ColouredLine> {
    let advance = width("0", mono());
    let answer = c(answer);
    // SAFETY: a NUL-terminated answer.
    let count = unsafe { nr_syntax_answer(answer.as_ptr()) };
    let mut lines = vec![ColouredLine::default()];
    for index in 0..count {
        let mut run: NrSyntaxRun = zeroed();
        // SAFETY: storage for one run.
        if !unsafe { nr_syntax_run(index, &mut run) } {
            continue;
        }
        let words = text(&run.text);
        if words == "\n" {
            lines.push(ColouredLine::default());
            continue;
        }
        let line = lines.last_mut().expect("one line at least");
        add_stroke(line, &words, run_ink(&run), tab, advance);
    }
    lines
}

/// A run's colour, `None` for the foreground.
fn run_ink(run: &NrSyntaxRun) -> Option<Hsla> {
    run.colored.then(|| {
        kit::display::shown(Rgba {
            r: run.color.red,
            g: run.color.green,
            b: run.color.blue,
            a: run.color.alpha,
        })
    })
}

/// Adds `words` in `ink` to the end of `line`.
fn add_stroke(line: &mut ColouredLine, words: &str, ink: Option<Hsla>, tab: usize, advance: f32) {
    let set = expand_tabs(words, line.text.chars().count(), tab);
    let x = x_of(&line.text, advance);
    line.text.push_str(&set);
    line.strokes.push(Stroke { text: set, x, ink });
}

// MARK: The screen

pub struct Changes {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The core's revision last loaded (`nr_changes_revision`).
    revision: Option<u64>,
    words: Words,
    laid: NrChangesFrames,
    files: Vec<File>,
    in_view: Option<usize>,
    /// Each shown file's rows and syntax colours, by its key.
    lines: HashMap<u64, Lines>,
    colours: HashMap<u64, Colours>,
    /// The highlighter's pending answers, by file key.
    colouring: HashMap<u64, Task<()>>,
    list_down: Scroller,
    down: Scroller,
    /// The diff offset the core's scroll-spy last heard.
    spied: Option<f32>,
    /// The pointer's window position while it is over the tab.
    pointer: Option<Point<Pixels>>,
    /// What the pointer is on, and the button pressed.
    hovered: Option<Target>,
    pressed: Option<Target>,
}

/// What a point over the diff is on (`nr_changes_hit`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Target {
    /// A tool of a file's name line.
    Tool(usize, NrChangesTool),
    /// A file's name line, off its tools.
    Line(usize),
    /// A tool of a hunk's strip.
    HunkTool(usize, NrChangesTool),
    /// A hunk's rows, off its strip.
    Hunk(usize),
}

impl Target {
    /// The file whose name line it is on.
    fn line(self) -> Option<usize> {
        match self {
            Self::Tool(file, _) | Self::Line(file) => Some(file),
            Self::HunkTool(..) | Self::Hunk(_) => None,
        }
    }

    /// The hunk it is on, whose strip shows.
    fn hunk(self) -> Option<usize> {
        match self {
            Self::HunkTool(hunk, _) | Self::Hunk(hunk) => Some(hunk),
            Self::Tool(..) | Self::Line(_) => None,
        }
    }

    /// A button: a tool.
    fn is_button(self) -> bool {
        match self {
            Self::Tool(..) | Self::HunkTool(..) => true,
            Self::Line(_) | Self::Hunk(_) => false,
        }
    }
}

impl Changes {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut changes = Self {
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            revision: None,
            words: Words::read(),
            laid: zeroed(),
            files: Vec::new(),
            in_view: None,
            lines: HashMap::new(),
            colours: HashMap::new(),
            colouring: HashMap::new(),
            list_down: Scroller::default(),
            down: Scroller::default(),
            spied: None,
            pointer: None,
            hovered: None,
            pressed: None,
        };
        changes.reload(window, cx);
        changes
    }

    /// Reads everything the tab shows from the core, keeping the file in view where it was.
    fn load(&mut self, cx: &mut Context<Self>) {
        // An action can leave its tab (Open selects the file's): it loads again once shown.
        if !nr_changes_shown() {
            self.revision = None;
            return;
        }
        self.revision = Some(nr_changes_revision());
        let loaded = nr_changes_load();
        self.files = (0..loaded.files)
            .filter_map(|index| File::read(index, &self.words))
            .collect();
        let keys: std::collections::HashSet<u64> = self.files.iter().map(|file| file.key).collect();
        self.lines.retain(|key, _| keys.contains(key));
        self.colours.retain(|key, _| keys.contains(key));
        self.colouring.retain(|key, _| keys.contains(key));
        self.in_view = usize::try_from(loaded.in_view).ok();
        self.lay_out(cx);
        self.down.scroll_to(loaded.offset as f32);
        self.spied = None;
        self.reveal_in_list();
        cx.notify();
    }

    /// Places the parts, and sets the diff's extent. What the pointer is
    /// on is found again, as the lines may have moved under it.
    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let size = self.frame.size;
        self.laid = nr_changes_frames(
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
            f64::from(width("0", mono())),
        );
        // The rows wrap anew for this width: read them again where they now fall.
        if self.laid.relaid {
            self.lines.clear();
            self.load(cx);
            return;
        }
        self.down
            .set_extent(self.laid.diff.height as f32, self.laid.extent as f32);
        let was = self.hovered.take();
        self.hover(cx);
        if was.is_some() && self.hovered.is_none() {
            cx.emit(Ask::Tooltip(None));
        }
    }

    /// Tells the core's scroll-spy where the diff is, once it moved; another file in view
    /// lights in the list and its name line is pinned.
    fn spy(&mut self, _: &mut Context<Self>) {
        let offset = self.down.offset();
        if self.files.is_empty() || self.spied == Some(offset) {
            return;
        }
        self.spied = Some(offset);
        let in_view = usize::try_from(nr_changes_scrolled(f64::from(offset))).ok();
        if in_view != self.in_view {
            self.in_view = in_view;
            self.reveal_in_list();
        }
    }

    /// Scrolls the list just far enough to show the file in view.
    fn reveal_in_list(&mut self) {
        let Some(index) = self.in_view else {
            return;
        };
        let (row, gap) = (self.laid.list_row as f32, self.laid.gap as f32);
        let list = self.laid.list.height as f32;
        self.list_down
            .set_extent(list, self.files.len() as f32 * row + 2.0 * gap);
        let top = gap + index as f32 * row;
        self.list_down.reveal(top - gap, top + row + gap);
    }

    /// Scrolls the diff to `offset`; the scroll-spy catches up as it draws.
    fn scroll_diff_to(&mut self, offset: f64, cx: &mut Context<Self>) {
        if offset < 0.0 {
            return;
        }
        self.down.scroll_to(offset as f32);
        self.spied = None;
        cx.notify();
    }

    /// Reads file `index`'s rows the first time it shows, and asks for its colours.
    fn read_lines(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self.files.get(index) else {
            return;
        };
        let key = file.key;
        if self.lines.contains_key(&key) {
            return;
        }
        self.lines.insert(key, Lines::read(index, file.rows));
        self.colour(index, key, cx);
    }

    /// Asks the highlighter for file `index`'s colours; they replace its plain lines once both
    /// sides are answered.
    fn colour(&mut self, index: usize, key: u64, cx: &mut Context<Self>) {
        let texts = nr_changes_texts(index);
        let extension = text(&texts.extension);
        let calls = [text(&texts.before_text), text(&texts.after_text)]
            .map(|side| syntax_call(&extension, &side));
        if calls.iter().all(String::is_empty) {
            return;
        }
        let answers = calls.map(|call| (!call.is_empty()).then(|| webhost::syntax::settle(&call)));
        let asked = cx.spawn(async move |changes, cx| {
            let mut said = [String::new(), String::new()];
            for (side, answer) in answers.into_iter().enumerate() {
                if let Some(answer) = answer {
                    said[side] = answer.await;
                }
            }
            let _ = changes.update(cx, |changes, cx| changes.coloured(key, &said, cx));
        });
        self.colouring.insert(key, asked);
    }

    /// Takes the highlighter's answers for each side of the file `key` names, if still listed.
    fn coloured(&mut self, key: u64, said: &[String; 2], cx: &mut Context<Self>) {
        if self.colouring.remove(&key).is_none() {
            return;
        }
        let tab = nr_source_tab_spaces() as usize;
        let mut colours = Colours::default();
        for (side, answer) in said.iter().enumerate() {
            if !answer.is_empty() {
                colours.sides[side] = coloured_lines(answer, tab);
            }
        }
        self.colours.insert(key, colours);
        cx.notify();
    }

    /// `part`, a frame of the tab, in window points.
    fn part(&self, part: &NrRect) -> Bounds<Pixels> {
        offset(
            rect(part),
            f32::from(self.frame.origin.x),
            f32::from(self.frame.origin.y),
        )
    }

    /// What window point `at` is on, with the frame of it in window points and its tooltip
    /// (empty for a name line or a hunk).
    fn hit(&self, at: Point<Pixels>) -> Option<(Target, Bounds<Pixels>, String)> {
        if self.files.is_empty() {
            return None;
        }
        let x = f32::from(at.x - self.frame.origin.x);
        let y = f32::from(at.y - self.frame.origin.y);
        let hit = nr_changes_hit(f64::from(self.down.offset()), f64::from(x), f64::from(y));
        let file = usize::try_from(hit.file).ok();
        let hunk = usize::try_from(hit.hunk).ok();
        let target = match (hit.on, file, hunk) {
            (NrChangesHitOn::Tool, Some(file), _) => Target::Tool(file, hit.tool),
            (NrChangesHitOn::Line, Some(file), _) => Target::Line(file),
            (NrChangesHitOn::HunkTool, _, Some(hunk)) => Target::HunkTool(hunk, hit.tool),
            (NrChangesHitOn::Hunk, _, Some(hunk)) => Target::Hunk(hunk),
            _ => return None,
        };
        Some((target, self.part(&hit.frame), text(&hit.tip)))
    }

    /// The tools of file `index`'s name line whose top is `top` in window points.
    fn line_tools(&self, index: usize, top: f32) -> NrChangesLineTools {
        let hovered = self.hovered.and_then(Target::line) == Some(index);
        let top = top - f32::from(self.frame.origin.y);
        nr_changes_line_tools(index, f64::from(top), hovered)
    }

    // MARK: Actions

    /// Reads the tab again after an action, and hands the core's reply to the window: a
    /// redraw, or a notice of why it failed.
    fn say(&mut self, effect: NrMenuEffect, cx: &mut Context<Self>) {
        if effect.effect == NrEffect::Changed {
            self.load(cx);
        }
        cx.emit(Ask::Said(Said::of(&effect)));
    }

    /// Does what the button `target` is does: a tool to its file or hunk.
    fn press_button(&mut self, target: Target, cx: &mut Context<Self>) {
        cx.emit(Ask::Tooltip(None));
        self.sync_spy();
        match target {
            Target::Tool(file, tool) => self.say(nr_changes_file_tool(file, tool), cx),
            Target::HunkTool(hunk, tool) => self.say(nr_changes_hunk_tool(hunk, tool), cx),
            Target::Line(_) | Target::Hunk(_) => {}
        }
    }

    // MARK: Keys and pointer

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sync_spy();
        let pressed = nr_changes_key(kit::keys::press(&event.keystroke));
        let effect = pressed.effect;
        if effect == NrChangesKeyEffect::PassOn {
            return;
        }
        self.follow(&pressed, cx);
        if effect == NrChangesKeyEffect::Changed {
            cx.emit(Ask::Changed);
        }
        if pressed.said.effect != NrEffect::Nothing {
            cx.emit(Ask::Said(Said::of(&pressed.said)));
        }
        cx.stop_propagation();
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if event.button != MouseButton::Left {
            return;
        }
        let hit = self.hit(event.position).map(|(target, _, _)| target);
        if let Some(button) = hit.filter(|target| target.is_button()) {
            self.pressed = Some(button);
            cx.emit(Ask::Tooltip(None));
            cx.notify();
            return;
        }
        let (list, diff) = (self.part(&self.laid.list), self.part(&self.laid.diff));
        if contains(&list, event.position) {
            self.press_list(event.position, list, cx);
        } else if contains(&diff, event.position) {
            if self.down.press(event.position, diff, now()) {
                cx.notify();
            } else if let Some(Target::Line(index)) = hit {
                self.fold(index, event.modifiers.alt, cx);
            }
        }
    }

    /// A press in the list: its scrollbar, else a file, whose section scrolls to the top.
    fn press_list(&mut self, at: Point<Pixels>, view: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.list_down.press(at, view, now()) {
            cx.notify();
            return;
        }
        if let Some(index) = self.listed_at(at) {
            self.sync_spy();
            let selected = nr_changes_select(index);
            self.follow(&selected, cx);
        }
    }

    /// Tells the core where the diff is now, so what it does next starts from there.
    fn sync_spy(&mut self) {
        if !self.files.is_empty() {
            nr_changes_scrolled(f64::from(self.down.offset()));
        }
    }

    /// Does what the core answered: load again, or scroll the diff.
    fn follow(&mut self, answer: &NrChangesKey, cx: &mut Context<Self>) {
        match answer.effect {
            NrChangesKeyEffect::PassOn => {}
            NrChangesKeyEffect::Changed => self.load(cx),
            NrChangesKeyEffect::Scroll => self.scroll_diff_to(answer.offset, cx),
        }
    }

    /// A press on file `index`'s name line, off its tools, folds it, with ⌥ every file.
    fn fold(&mut self, index: usize, all: bool, cx: &mut Context<Self>) {
        self.sync_spy();
        if nr_changes_fold(index, all) {
            self.load(cx);
        }
    }

    /// The list row under `at`, if any.
    fn listed_at(&self, at: Point<Pixels>) -> Option<usize> {
        let list = self.part(&self.laid.list);
        let y = f32::from(at.y - list.origin.y) + self.list_down.offset() - self.laid.gap as f32;
        let index = (y / self.laid.list_row as f32).floor();
        (index >= 0.0 && (index as usize) < self.files.len()).then_some(index as usize)
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, over: bool, cx: &mut Context<Self>) {
        let now = now();
        let (list, diff) = (self.part(&self.laid.list), self.part(&self.laid.diff));
        if self.list_down.is_held() {
            self.list_down.pointer(event.position, list, now);
        } else if self.down.is_held() {
            self.down.pointer(event.position, diff, now);
        } else {
            self.list_down.pointer(event.position, list, now);
            self.down.pointer(event.position, diff, now);
        }
        let pointer = over.then_some(event.position);
        if pointer != self.pointer || self.list_down.is_held() || self.down.is_held() {
            self.pointer = pointer;
            cx.notify();
        }
        self.hover(cx);
    }

    /// What the pointer is on now: a line shows its Keep and Undo, a hunk its strip, and a
    /// button its tooltip.
    fn hover(&mut self, cx: &mut Context<Self>) {
        let held = self.list_down.is_held() || self.down.is_held();
        let hit = self.pointer.filter(|_| !held).and_then(|at| self.hit(at));
        let hovered = hit.as_ref().map(|(target, _, _)| *target);
        if hovered == self.hovered {
            return;
        }
        self.hovered = hovered;
        let tip = hit
            .filter(|(target, _, tip)| target.is_button() && !tip.is_empty())
            .map(|(_, at, tip)| (at, tip));
        cx.emit(Ask::Tooltip(tip));
        cx.notify();
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let now = now();
        if let Some(pressed) = self.pressed.take() {
            cx.notify();
            if self.hit(event.position).map(|(target, _, _)| target) == Some(pressed) {
                self.press_button(pressed, cx);
            }
        }
        if self.list_down.release(now) | self.down.release(now) {
            cx.notify();
        }
    }

    /// A wheel over the list scrolls it; over the diff it scrolls the diff. Long lines wrap, so
    /// neither scrolls sideways.
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let now = now();
        let moved = if contains(&self.part(&self.laid.list), event.position) {
            self.list_down.wheel(event, now)
        } else if contains(&self.part(&self.laid.diff), event.position) {
            self.down.wheel(event, now)
        } else {
            false
        };
        if moved {
            self.hover(cx);
            cx.notify();
        }
    }

    /// Registers pointer handlers while the tab is drawn: over the tab, and anywhere in the
    /// window while one of its scrollbars or tools is held.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let changes = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            changes.update(cx, |changes, cx| changes.mouse_down(event, window, cx));
        });
        let (changes, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
            let hovered = over.is_hovered(window);
            changes.update(cx, |changes, cx| changes.mouse_move(event, hovered, cx));
        });
        let changes = entity.clone();
        on_bubble(window, move |event: &MouseUpEvent, _, cx| {
            if event.button == MouseButton::Left {
                changes.update(cx, |changes, cx| changes.mouse_up(event, cx));
            }
        });
        let changes = entity;
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            changes.update(cx, |changes, cx| changes.scroll(event, cx));
        });
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        fill(window, self.frame, color(Token::Background));
        self.paint_list(window, cx, now);
        if self.files.is_empty() {
            paint_note(window, cx, &self.words.empty, self.part(&self.laid.diff));
        } else {
            self.spy(cx);
            self.paint_diff(window, cx, now);
        }
        if self.list_down.tick(now) | self.down.tick(now) {
            window.request_animation_frame();
        }
        self.listen(hitbox, window, cx);
    }

    /// The list's rows and its right edge.
    fn paint_list(&mut self, window: &mut Window, cx: &mut App, now: f64) {
        let view = self.part(&self.laid.list);
        let (row, gap) = (self.laid.list_row as f32, self.laid.gap as f32);
        self.list_down.set_extent(
            f32::from(view.size.height),
            self.files.len() as f32 * row + 2.0 * gap,
        );
        let edge = frame(
            f32::from(view.origin.x + view.size.width) - 1.0,
            f32::from(view.origin.y),
            1.0,
            f32::from(view.size.height),
        );
        fill(window, edge, color(Token::Border));
        let top = f32::from(view.origin.y) + gap - self.list_down.offset();
        window.with_content_mask(Some(ContentMask { bounds: view }), |window| {
            for (index, file) in self.files.iter().enumerate() {
                let at = frame(
                    f32::from(view.origin.x),
                    top + index as f32 * row,
                    f32::from(view.size.width) - 1.0,
                    row,
                );
                if at.origin.y + at.size.height < view.origin.y || at.origin.y > view.bottom() {
                    continue;
                }
                let lit = self.pointer.is_some_and(|pointer| contains(&at, pointer));
                let in_view = self.in_view == Some(index);
                paint_listed(window, cx, file, at, in_view, lit);
            }
            self.list_down.paint_bar(window, view, now);
        });
    }

    /// The files whose sections show in a view `shown` tall at the current scroll.
    fn shown_files(&self, shown: f32) -> std::ops::Range<usize> {
        let offset = self.down.offset();
        let first = self
            .files
            .partition_point(|file| file.top + file.height <= offset);
        let end = self.files.partition_point(|file| file.top < offset + shown);
        first..end.max(first)
    }

    /// Every section on screen: its name line, then its rows or its block.
    fn paint_diff(&mut self, window: &mut Window, cx: &mut Context<Self>, now: f64) {
        let view = self.part(&self.laid.diff);
        let shown = self.shown_files(f32::from(view.size.height));
        for index in shown.clone() {
            self.read_lines(index, cx);
        }
        window.with_content_mask(Some(ContentMask { bounds: view }), |window| {
            for index in shown {
                self.paint_section(window, cx, index, view);
            }
            self.paint_strips(window, cx);
            self.paint_pinned(window, cx, view);
            self.down.paint_bar(window, view, now);
        });
    }

    /// File `index`'s section in `view` at the current scroll.
    fn paint_section(&self, window: &mut Window, cx: &mut App, index: usize, view: Bounds<Pixels>) {
        let file = &self.files[index];
        let top = f32::from(view.origin.y) + file.top - self.down.offset();
        self.paint_separator(window, cx, index, index > 0, top, view);
        if file.folded {
            return;
        }
        let body = top + self.laid.separator as f32;
        match self
            .lines
            .get(&file.key)
            .filter(|lines| !lines.rows.is_empty())
        {
            Some(lines) => self.paint_rows(window, cx, file, lines, body, view),
            None => self.paint_block(window, cx, file, body, view),
        }
    }

    /// The pinned name line, when the diff is scrolled into its file's section: drawn over
    /// the rows under it on the background, a hairline under it, and pushed up by the next
    /// file's line, whose own hairline then marks the edge.
    fn paint_pinned(&self, window: &mut Window, cx: &mut App, view: Bounds<Pixels>) {
        let pinned = nr_changes_pinned(f64::from(self.down.offset()));
        let Some(index) = usize::try_from(pinned.file)
            .ok()
            .filter(|_| pinned.scrolled)
        else {
            return;
        };
        let top = f32::from(self.frame.origin.y) + pinned.top as f32;
        let (x, wide) = (f32::from(view.origin.x), f32::from(view.size.width));
        let height = self.laid.separator as f32;
        fill(
            window,
            frame(x, top, wide, height),
            color(Token::Background),
        );
        self.paint_separator(window, cx, index, false, top, view);
        // While pushed, the next line's own hairline sits right under it; one rule, not two.
        if top >= f32::from(view.origin.y) {
            let rule = frame(x, top + height - 1.0, wide, 1.0);
            fill(window, rule, color(Token::Border));
        }
    }

    /// File `index`'s name line from `top`: a hairline over it (`ruled`), a chevron (down when
    /// expanded), its path small and muted, then a note when it was too different to compare,
    /// or once collapsed its row's ending; its tools at the right end. Shaded under the
    /// pointer, as a click folds it.
    fn paint_separator(
        &self,
        window: &mut Window,
        cx: &mut App,
        index: usize,
        ruled: bool,
        top: f32,
        view: Bounds<Pixels>,
    ) {
        let file = &self.files[index];
        let (x, wide) = (f32::from(view.origin.x), f32::from(view.size.width));
        let height = self.laid.separator as f32;
        let line = frame(x, top, wide, height);
        let over = self.hovered.and_then(Target::line) == Some(index)
            && self.pointer.is_some_and(|pointer| contains(&line, pointer));
        if over {
            fill(window, line, mixed(Token::Foreground, HOVER));
        }
        if ruled {
            fill(window, frame(x, top, wide, 1.0), color(Token::Border));
        }
        let tools = self.line_tools(index, top);
        let inset = self.laid.inset as f32;
        let gap = self.laid.gap as f32;
        let y = top + (height - 16.0) / 2.0;
        let muted = color(Token::MutedForeground);
        let chevron = match file.folded {
            true => Glyph::CaretRight,
            false => Glyph::CaretDown,
        };
        let mut left = x + inset - 4.0;
        icons::draw(window, cx, chevron, frame(left, y + 1.0, 14.0, 14.0), muted);
        left += 14.0 + 4.0;
        let end = f32::from(self.frame.origin.x) + tools.text_end as f32;
        let room = (end - left).max(0.0);
        draw_line(
            window,
            cx,
            &file.path,
            path_face(),
            muted,
            left,
            y,
            16.0,
            room,
            None,
        );
        let after = left + width(&file.path, path_face()) + gap;
        let room = (end - after).max(0.0);
        if file.folded {
            paint_summary(window, cx, &file.ending, muted, after, y, room);
        } else if file.whole {
            let note = faded(muted, 0.8);
            draw_line(
                window,
                cx,
                &self.words.whole,
                path_face(),
                note,
                after,
                y,
                16.0,
                room,
                None,
            );
        }
        self.paint_line_tools(window, cx, index, &tools);
    }

    /// A name line's tools: muted icons that darken under the pointer.
    fn paint_line_tools(
        &self,
        window: &mut Window,
        cx: &mut App,
        index: usize,
        tools: &NrChangesLineTools,
    ) {
        for button in &tools.tools[..tools.count.min(tools.tools.len())] {
            let at = self.part(&button.frame);
            let target = Some(Target::Tool(index, button.tool));
            let lit = self.hovered == target;
            let shade = match (self.pressed == target, lit) {
                (true, _) => Some(PRESS),
                (false, true) => Some(HOVER),
                (false, false) => None,
            };
            if let Some(shade) = shade {
                rounded(window, at, mixed(Token::Foreground, shade), TOOL_CORNER);
            }
            let ink = match lit {
                true => color(Token::Foreground),
                false => color(Token::MutedForeground),
            };
            paint_glyph(window, cx, &text(&button.glyph), at, ink);
        }
    }

    /// The rows of `file` that show, from `body` down, each over as many visual lines as it
    /// wraps onto.
    fn paint_rows(
        &self,
        window: &mut Window,
        cx: &mut App,
        file: &File,
        lines: &Lines,
        body: f32,
        view: Bounds<Pixels>,
    ) {
        let line = self.laid.diff_line as f32;
        let above = ((f32::from(view.origin.y) - body) / line).floor().max(0.0) as usize;
        let below = f32::from(view.origin.y + view.size.height);
        let first = lines
            .rows
            .partition_point(|row| row.line + row.height() <= above);
        let colours = self.colours.get(&file.key);
        for row in &lines.rows[first.min(lines.rows.len())..] {
            let top = body + row.line as f32 * line;
            if top >= below {
                break;
            }
            let at = frame(
                f32::from(view.origin.x),
                top,
                f32::from(view.size.width),
                line * row.height() as f32,
            );
            self.paint_row(window, cx, row, file.gutter, colours, lines.advance, at);
        }
    }

    /// One row in `at`: its ground, line numbers on its first line, changed words and text,
    /// each of its visual lines showing the stretch of the text the core wrapped onto it.
    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        window: &mut Window,
        cx: &mut App,
        row: &Row,
        gutter: f32,
        colours: Option<&Colours>,
        advance: f32,
        at: Bounds<Pixels>,
    ) {
        if let Some(ground) = row.ground() {
            fill(window, at, ground);
        }
        let (x, top) = (f32::from(at.origin.x), f32::from(at.origin.y));
        let line = self.laid.diff_line as f32;
        paint_numbers(window, cx, row, frame(x, top, gutter, line));
        let text_x = x + gutter * 2.0 + self.laid.gap as f32;
        let room = f32::from(at.size.width) - (text_x - x);
        let coloured = colours.and_then(|colours| colours.line(row));
        for (index, from) in row.wraps.iter().enumerate() {
            let to = row.wraps.get(index + 1).copied().unwrap_or(f32::MAX);
            let y = top + index as f32 * line;
            let shown = frame(text_x, y, (to - from).min(room).max(0.0), line);
            let start = text_x - from;
            window.with_content_mask(Some(ContentMask { bounds: shown }), |window| {
                for (left, right) in &row.words {
                    fill(
                        window,
                        frame(start + left, y, right - left, line),
                        row.word_ground(),
                    );
                }
                let span = Span {
                    from: *from,
                    to,
                    advance,
                };
                paint_text(window, cx, row, coloured, span, start, y);
            });
        }
    }

    /// The block of a file without diff lines, from `body`: its name and what happened.
    fn paint_block(
        &self,
        window: &mut Window,
        cx: &mut App,
        file: &File,
        body: f32,
        view: Bounds<Pixels>,
    ) {
        let inset = self.laid.inset as f32;
        let x = f32::from(view.origin.x) + inset;
        let wide = (f32::from(view.size.width) - 2.0 * inset).clamp(0.0, 360.0);
        let block = frame(x, body, wide, 44.0);
        stroke(
            window,
            inset_by(block, 0.5),
            color(Token::Border),
            BLOCK_CORNER - 0.5,
            1.0,
        );
        let name_face = Face::sans(13.0, 600.0);
        let room = wide - 2.0 * 12.0;
        let ink = color(Token::Foreground);
        let left = x + 12.0;
        draw_line(
            window,
            cx,
            &file.name,
            name_face,
            ink,
            left,
            body + 5.0,
            18.0,
            room,
            None,
        );
        let words = self.words.happened(file.status);
        let muted = color(Token::MutedForeground);
        draw_line(
            window,
            cx,
            words,
            path_face(),
            muted,
            left,
            body + 23.0,
            16.0,
            room,
            None,
        );
    }

    /// The strips of the hunk under the pointer and of the one N or P stepped to.
    fn paint_strips(&self, window: &mut Window, cx: &mut App) {
        let hovered = self.hovered.and_then(Target::hunk);
        let stepped = usize::try_from(nr_changes_current_hunk()).ok();
        let offset = f64::from(self.down.offset());
        for hunk in [stepped, hovered.filter(|hunk| Some(*hunk) != stepped)]
            .into_iter()
            .flatten()
        {
            let strip = nr_changes_strip(hunk, offset);
            if strip.shown {
                self.paint_strip(window, cx, hunk, &strip);
            }
        }
    }

    /// A hunk's strip: a small rounded ground with a hairline, its tools as muted icons that
    /// darken under the pointer.
    fn paint_strip(&self, window: &mut Window, cx: &mut App, hunk: usize, strip: &NrChangesStrip) {
        let at = self.part(&strip.frame);
        rounded(window, at, color(Token::Background), STRIP_CORNER);
        stroke(
            window,
            inset_by(at, 0.5),
            color(Token::Border),
            STRIP_CORNER - 0.5,
            1.0,
        );
        for button in &strip.tools[..strip.count.min(strip.tools.len())] {
            let place = self.part(&button.frame);
            let target = Some(Target::HunkTool(hunk, button.tool));
            let lit = self.hovered == target;
            let shade = match (self.pressed == target, lit) {
                (true, _) => Some(PRESS),
                (false, true) => Some(HOVER),
                (false, false) => None,
            };
            if let Some(shade) = shade {
                rounded(
                    window,
                    inset(place, 1.0, 1.0),
                    mixed(Token::Foreground, shade),
                    STRIP_CORNER,
                );
            }
            let ink = match lit {
                true => color(Token::Foreground),
                false => color(Token::MutedForeground),
            };
            paint_glyph_sized(window, cx, &text(&button.glyph), place, ink, STRIP_GLYPH);
        }
    }
}

/// The glyph `name` names, `GLYPH` square, centred in `at`.
fn paint_glyph(window: &mut Window, cx: &mut App, name: &str, at: Bounds<Pixels>, ink: Hsla) {
    paint_glyph_sized(window, cx, name, at, ink, GLYPH);
}

/// The glyph `name` names, `side` square, centred in `at`.
fn paint_glyph_sized(
    window: &mut Window,
    cx: &mut App,
    name: &str,
    at: Bounds<Pixels>,
    ink: Hsla,
    side: f32,
) {
    let Some(glyph) = icons::named(name) else {
        return;
    };
    let x = f32::from(at.origin.x) + (f32::from(at.size.width) - side) / 2.0;
    let y = f32::from(at.origin.y) + (f32::from(at.size.height) - side) / 2.0;
    icons::draw(window, cx, glyph, frame(x, y, side, side), ink);
}

/// `bounds` shrunk by `by` on every side.
fn inset_by(bounds: Bounds<Pixels>, by: f32) -> Bounds<Pixels> {
    inset(bounds, by, by)
}

/// The stretch of a row's text one visual line shows: x from the text's start, `from` up to
/// `to`, in a monospace face whose figures are `advance` wide.
#[derive(Clone, Copy)]
struct Span {
    from: f32,
    to: f32,
    advance: f32,
}

impl Span {
    /// The part of `text`, set from `x`, that starts within the span, and the x it starts at:
    /// only that part is shaped and drawn, not the whole row once per visual line.
    fn part<'a>(&self, text: &'a str, x: f32) -> Option<(&'a str, f32)> {
        // Half a figure of slack, so a figure placed on the boundary falls on one side.
        let slack = self.advance / 2.0;
        let mut at = x;
        let (mut first, mut end) = (None, text.len());
        for (byte, character) in text.char_indices() {
            if at >= self.to - slack {
                end = byte;
                break;
            }
            if first.is_none() && at >= self.from - slack {
                first = Some((byte, at));
            }
            at += northroom_core::changes::diff::cells(character.encode_utf8(&mut [0; 4])) as f32
                * self.advance;
        }
        let (start, placed) = first?;
        (start < end).then(|| (&text[start..end], placed))
    }
}

/// The stretch `span` of a row's text, the text set from `start`: in its syntax colours once
/// answered (`coloured`), else in one ink, a hunk header muted.
fn paint_text(
    window: &mut Window,
    cx: &mut App,
    row: &Row,
    coloured: Option<&ColouredLine>,
    span: Span,
    start: f32,
    top: f32,
) {
    let foreground = color(Token::Foreground);
    let plain = match row.shows {
        NrChangesRowShows::Header => color(Token::MutedForeground),
        _ => foreground,
    };
    let whole = [(row.text.as_str(), 0.0, plain)];
    let strokes: Vec<(&str, f32, Hsla)> = match coloured {
        Some(coloured) => coloured
            .strokes
            .iter()
            .map(|stroke| {
                (
                    stroke.text.as_str(),
                    stroke.x,
                    stroke.ink.unwrap_or(foreground),
                )
            })
            .collect(),
        None => whole.to_vec(),
    };
    for (text, x, ink) in strokes {
        if let Some((part, placed)) = span.part(text, x) {
            draw_line(
                window,
                cx,
                part,
                mono(),
                ink,
                start + placed,
                top,
                TEXT_LINE,
                f32::MAX,
                None,
            );
        }
    }
}

/// The line box a diff row's text is set in.
const TEXT_LINE: f32 = 16.0;

/// A row's two line numbers, right-aligned in their columns from `at`, muted.
fn paint_numbers(window: &mut Window, cx: &mut App, row: &Row, at: Bounds<Pixels>) {
    let (x, top) = (f32::from(at.origin.x), f32::from(at.origin.y));
    let (gutter, line) = (f32::from(at.size.width), f32::from(at.size.height));
    let ink = faded(color(Token::MutedForeground), 0.8);
    for (column, number) in [row.old, row.new].into_iter().enumerate() {
        if number == 0 {
            continue;
        }
        let words = number.to_string();
        let right = x + (column + 1) as f32 * gutter - 8.0;
        let left = right - width(&words, mono());
        draw_line(
            window,
            cx,
            &words,
            mono(),
            ink,
            left,
            top,
            line,
            gutter,
            None,
        );
    }
}

/// A sentence centred in `view`, muted.
fn paint_note(window: &mut Window, cx: &mut App, words: &str, view: Bounds<Pixels>) {
    let face = Face::sans(14.0, 400.0);
    let wide = width(words, face);
    let x = f32::from(view.origin.x) + (f32::from(view.size.width) - wide) / 2.0;
    let y = f32::from(view.origin.y) + (f32::from(view.size.height) - 20.0) / 2.0;
    let ink = color(Token::MutedForeground);
    draw_line(window, cx, words, face, ink, x, y, 20.0, wide + 1.0, None);
}

/// One file row of the list in `at`, lit when it is in view, shaded under the pointer.
fn paint_listed(
    window: &mut Window,
    cx: &mut App,
    file: &File,
    at: Bounds<Pixels>,
    in_view: bool,
    lit: bool,
) {
    let bar = inset(at, 8.0, 2.0);
    if in_view {
        rounded(window, bar, color(Token::Accent), 6.0);
    } else if lit {
        rounded(window, bar, mixed(Token::Foreground, HOVER), 6.0);
    }
    let right = f32::from(bar.origin.x + bar.size.width) - 8.0;
    let right = paint_ending(window, cx, &file.ending, right, bar);
    let mut x = f32::from(bar.origin.x) + 8.0;
    let y = f32::from(bar.origin.y) + (f32::from(bar.size.height) - 16.0) / 2.0;
    if file.contested {
        let glyph = frame(x, y + 1.0, 14.0, 14.0);
        icons::draw(
            window,
            cx,
            Glyph::Warning,
            glyph,
            color(Token::StatusWarning),
        );
        x += 14.0 + 6.0;
    }
    let ink = match in_view {
        true => color(Token::AccentForeground),
        false => color(Token::Foreground),
    };
    let room = (right - 8.0 - x).max(0.0);
    let clip = ContentMask {
        bounds: frame(x, f32::from(bar.origin.y), room, f32::from(bar.size.height)),
    };
    window.with_content_mask(Some(clip), |window| {
        draw_line(
            window,
            cx,
            &file.path,
            path_face(),
            ink,
            x,
            y,
            16.0,
            room,
            None,
        );
    });
}

/// The words of a row's ending: "+3" and "−1" in the status colours, leaving out a count of
/// zero, or its status's word, muted.
fn ending_parts(ending: &Ending) -> Vec<(String, Token)> {
    match ending {
        Ending::Counts(plus, minus) => [
            (*plus, '+', Token::StatusSuccess),
            (*minus, '−', Token::Destructive),
        ]
        .into_iter()
        .filter(|(count, _, _)| *count > 0)
        .map(|(count, sign, token)| (format!("{sign}{count}"), token))
        .collect(),
        Ending::Word(word) => vec![(word.clone(), Token::MutedForeground)],
        Ending::Nothing => Vec::new(),
    }
}

fn ending_face() -> Face {
    Face::sans(11.0, 500.0).tabular()
}

/// A file row's ending, right-aligned at `right` in `bar`. Returns where it starts.
fn paint_ending(
    window: &mut Window,
    cx: &mut App,
    ending: &Ending,
    right: f32,
    bar: Bounds<Pixels>,
) -> f32 {
    let face = ending_face();
    let y = f32::from(bar.origin.y) + (f32::from(bar.size.height) - 16.0) / 2.0;
    let mut x = right;
    for (words, token) in ending_parts(ending).into_iter().rev() {
        let ink = color(token);
        let wide = width(&words, face);
        x -= wide;
        draw_line(window, cx, &words, face, ink, x, y, 16.0, wide + 1.0, None);
        x -= 4.0;
    }
    x
}

/// A collapsed file's ending after its name, from `x`, in `ink`, within `room`: "+3 −1", or
/// its status's word.
fn paint_summary(
    window: &mut Window,
    cx: &mut App,
    ending: &Ending,
    ink: Hsla,
    x: f32,
    y: f32,
    room: f32,
) {
    let words: Vec<String> = ending_parts(ending)
        .into_iter()
        .map(|(words, _)| words)
        .collect();
    let words = words.join(" ");
    draw_line(
        window,
        cx,
        &words,
        ending_face(),
        ink,
        x,
        y,
        16.0,
        room,
        None,
    );
}

impl EventEmitter<Ask> for Changes {}

impl Screen for Changes {
    /// Reads the tab again when the core's changes moved on.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.revision != Some(nr_changes_revision()) {
            self.load(cx);
        }
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            self.lay_out(cx);
            self.spied = None;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "ChangesView".to_owned();
        }
    }

    /// The centre of the file in view's tool whose tooltip is `title`, or of the list row of
    /// the file at that path.
    fn point_of(&self, title: &str, _: &App) -> Option<Point<Pixels>> {
        if let Some(at) = self.in_view_tool(title) {
            return Some(at);
        }
        let index = self.files.iter().position(|file| file.path == title)?;
        let list = self.part(&self.laid.list);
        let row = self.laid.list_row as f32;
        let top = f32::from(list.origin.y) + self.laid.gap as f32 - self.list_down.offset();
        Some(point(
            list.origin.x + list.size.width / 2.0,
            px(top + (index as f32 + 0.5) * row),
        ))
    }

    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        let (list, diff) = (self.part(&self.laid.list), self.part(&self.laid.diff));
        let gap = self.laid.gap as f32;
        if contains(&list, at) {
            let length = self.files.len() as f32 * self.laid.list_row as f32 + 2.0 * gap;
            return Some((self.list_down.offset(), f32::from(list.size.height), length));
        }
        contains(&diff, at).then(|| {
            let shown = f32::from(diff.size.height);
            (self.down.offset(), shown, self.laid.extent as f32)
        })
    }
}

impl Render for Changes {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let changes = cx.entity();
        kit::controls::placed("changes", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        changes.update(cx, |changes, cx| changes.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Route::Kind(NrScriptKind::ChangesSay), said_changes);
}

/// `changessay`: one line per listed file (`changes\trow\t<path>\t<ending>\t<flags>`, flags
/// `contested`, `inview` and `folded`), the diff's offset and the pinned file
/// (`changes\tview\t<offset>\t<path>`), each file's section
/// (`changes\tfile\t<path>\t<top>\t<rows>\t<coloured>\t<height>`), and each of its drawn rows
/// (`changes\tline\t<path>\t<shows>\t<old>\t<new>\t<text>`), with `\tcoloured` when it shows in
/// syntax colours, then `\twraps:<n>`, the visual lines it wraps onto.
fn said_changes(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    window.draw(cx).clear(cx);
    let Some(changes) = workspace(window).read(cx).shown_changes().cloned() else {
        eprintln!("changes\tnot shown");
        return Answer::Done;
    };
    for line in changes.read(cx).said() {
        eprintln!("{line}");
    }
    Answer::Done
}

impl Changes {
    /// The centre of the tool of the file in view's name line whose tooltip is `title`, its
    /// line where it shows (pinned at the top once scrolled into), Keep and Undo included.
    fn in_view_tool(&self, title: &str) -> Option<Point<Pixels>> {
        let index = self.in_view?;
        let pinned = nr_changes_pinned(f64::from(self.down.offset()));
        let tools = nr_changes_line_tools(index, pinned.top, true);
        tools.tools[..tools.count.min(tools.tools.len())]
            .iter()
            .find(|button| text(&button.tip) == title)
            .map(|button| self.part(&button.frame).center())
    }

    /// What `changessay` reports, line by line.
    fn said(&self) -> Vec<String> {
        let mut said: Vec<String> = self
            .files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let ending = match &file.ending {
                    Ending::Counts(plus, minus) => format!("+{plus} -{minus}"),
                    Ending::Word(word) => word.clone(),
                    Ending::Nothing => String::new(),
                };
                let flags = [
                    (file.contested, "contested"),
                    (self.in_view == Some(index), "inview"),
                    (file.folded, "folded"),
                ]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(",");
                format!("changes\trow\t{}\t{ending}\t{flags}", file.path)
            })
            .collect();
        let pinned = nr_changes_pinned(f64::from(self.down.offset()));
        let pinned = usize::try_from(pinned.file)
            .ok()
            .and_then(|index| self.files.get(index))
            .map_or("", |file| file.path.as_str());
        said.push(format!("changes\tview\t{}\t{pinned}", self.down.offset()));
        for (index, file) in self.files.iter().enumerate() {
            said.extend(self.said_file(index, file));
        }
        said
    }

    /// A file's section and rows for `changessay`, read now if it never showed.
    fn said_file(&self, index: usize, file: &File) -> Vec<String> {
        let read;
        let lines = match self.lines.get(&file.key) {
            Some(lines) => lines,
            None => {
                read = Lines::read(index, file.rows);
                &read
            }
        };
        let colours = self.colours.get(&file.key);
        let coloured = colours.map_or(0, |colours| {
            colours
                .sides
                .iter()
                .flatten()
                .filter(|line| line.strokes.iter().any(|stroke| stroke.ink.is_some()))
                .count()
        });
        let head = format!(
            "changes\tfile\t{}\t{}\t{}\t{coloured}\t{}",
            file.path, file.top, file.rows, file.height
        );
        let rows = lines.rows.iter().map(|row| {
            let coloured = match colours.and_then(|colours| colours.line(row)) {
                Some(_) => "\tcoloured",
                None => "",
            };
            format!(
                "changes\tline\t{}\t{:?}\t{}\t{}\t{}{coloured}\twraps:{}",
                file.path,
                row.shows,
                row.old,
                row.new,
                row.text,
                row.height()
            )
        });
        std::iter::once(head).chain(rows).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ending: Ending) -> Vec<String> {
        ending_parts(&ending)
            .into_iter()
            .map(|(words, _)| words)
            .collect()
    }

    #[test]
    fn a_count_of_zero_is_left_out() {
        assert_eq!(words(Ending::Counts(3, 1)), ["+3", "−1"]);
        assert_eq!(words(Ending::Counts(1, 0)), ["+1"]);
        assert_eq!(words(Ending::Counts(0, 2)), ["−2"]);
        assert!(words(Ending::Counts(0, 0)).is_empty());
    }
}
