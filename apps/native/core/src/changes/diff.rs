//! Line diffs for the Changes tab: Myers' O(ND) diff with a deadline, hunks with three lines of
//! context, words marked inside paired changed lines, and the rows of the unified diff, so the
//! shell only draws. Past the deadline, or with too many edits to trace, the
//! whole file counts as changed.

use crate::changes::Clock;
use std::collections::HashMap;
use std::ops::Range;

/// Lines of unchanged text kept around each change.
pub const CONTEXT_LINES: usize = 3;
/// How long one file's diff may take before the whole file counts as changed.
pub const DEADLINE_MS: u64 = 50;
/// The most edits traced; the trace grows with their square.
const MOST_EDITS: usize = 2048;
/// How many edit steps run between looks at the clock.
const STEPS_PER_LOOK: isize = 16;

/// When a diff gives up.
pub struct Deadline<'a> {
    pub clock: &'a dyn Clock,
    pub until_ms: u64,
}

impl Deadline<'_> {
    fn passed(&self) -> bool {
        self.clock.now_ms() > self.until_ms
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same,
    Removed,
    Added,
}

/// What a line of a diff is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of a hunk. `text` keeps its line ending; `shown` is without it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// 1-based line numbers in the old and new file.
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
    /// Byte ranges of `shown()` that changed within a paired line.
    pub words: Vec<Range<usize>>,
}

impl Line {
    /// The line as drawn, without its ending.
    pub fn shown(&self) -> &str {
        without_ending(&self.text)
    }
}

/// A run of changes with its context. Starts are 1-based; a side with no lines starts at the
/// line before, as in a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub lines: Vec<Line>,
}

impl Hunk {
    /// Its header, as in a unified diff.
    pub fn header(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@",
            self.old_start, self.old_count, self.new_start, self.new_count
        )
    }

    /// Its lines on one side, with their endings: context and that side's changes, in order.
    pub fn side(&self, side: Side) -> Vec<&str> {
        let wanted = match side {
            Side::Old => LineKind::Removed,
            Side::New => LineKind::Added,
        };
        self.lines
            .iter()
            .filter(|line| line.kind == LineKind::Context || line.kind == wanted)
            .map(|line| line.text.as_str())
            .collect()
    }

    /// The index of the first line of `side` it covers: a side with no lines starts at the line
    /// before, so its lines go after that one.
    fn first_index(&self, side: Side) -> usize {
        let (start, count) = match side {
            Side::Old => (self.old_start, self.old_count),
            Side::New => (self.new_start, self.new_count),
        };
        match count {
            0 => start,
            _ => start - 1,
        }
    }
}

/// A side of a diff: the text it was taken from, or the text it was taken to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Old,
    New,
}

/// `old`, the text `hunk` was diffed from, with the hunk taken in: its old lines replaced by
/// its new ones, so the two texts no longer differ there (Keep). `None` when `old` no longer
/// holds the hunk's old lines where it says.
pub fn keep_hunk(old: &str, hunk: &Hunk) -> Option<String> {
    let lines = split_lines(old);
    let (old_side, new_side) = (hunk.side(Side::Old), hunk.side(Side::New));
    let at = hunk.first_index(Side::Old);
    let held = lines.get(at..at + old_side.len())?;
    (held == old_side.as_slice()).then(|| splice(&lines, at, old_side.len(), &new_side))
}

/// `current`, a file's text now, with `hunk` undone: its new lines, found nearest where the
/// hunk put them, replaced by its old ones (Undo). `None` when the file no longer holds them.
pub fn undo_hunk(current: &str, hunk: &Hunk) -> Option<String> {
    let lines = split_lines(current);
    let (old_side, new_side) = (hunk.side(Side::Old), hunk.side(Side::New));
    let at = find_lines(&lines, &new_side, hunk.first_index(Side::New))?;
    Some(splice(&lines, at, new_side.len(), &old_side))
}

/// `lines` with the `count` from `at` replaced by `with`, joined.
fn splice(lines: &[&str], at: usize, count: usize, with: &[&str]) -> String {
    let mut joined: Vec<&str> = lines[..at].to_vec();
    joined.extend(with);
    joined.extend(&lines[at + count..]);
    joined.concat()
}

/// Where `wanted` appears in `lines`, nearest `expected`.
fn find_lines(lines: &[&str], wanted: &[&str], expected: usize) -> Option<usize> {
    let last = lines.len().checked_sub(wanted.len())?;
    let fits = |at: usize| lines[at..at + wanted.len()] == *wanted;
    (0..=last)
        .filter(|&at| fits(at))
        .min_by_key(|&at| at.abs_diff(expected))
}

/// A whole file's diff.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDiff {
    pub hunks: Vec<Hunk>,
    /// The deadline passed, so every old line is removed and every new one added.
    pub whole: bool,
}

impl FileDiff {
    /// Lines added and removed.
    pub fn counts(&self) -> (usize, usize) {
        let lines = self.hunks.iter().flat_map(|hunk| &hunk.lines);
        lines.fold((0, 0), |(plus, minus), line| match line.kind {
            LineKind::Added => (plus + 1, minus),
            LineKind::Removed => (plus, minus + 1),
            LineKind::Context => (plus, minus),
        })
    }
}

/// Splits text into lines that keep their endings. An empty text has none; a last line without
/// an ending is a line.
pub fn split_lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn without_ending(line: &str) -> &str {
    let line = line.strip_suffix('\n').unwrap_or(line);
    line.strip_suffix('\r').unwrap_or(line)
}

/// The diff from `old` to `new`.
pub fn diff(old: &str, new: &str, deadline: &Deadline) -> FileDiff {
    let (old_lines, new_lines) = (split_lines(old), split_lines(new));
    let (a, b) = intern(&old_lines, &new_lines);
    let (ops, whole) = match edit_script(&a, &b, deadline) {
        Some(ops) => (ops, false),
        None => (whole_change(a.len(), b.len()), true),
    };
    let mut hunks = build_hunks(&ops, &old_lines, &new_lines);
    if !whole {
        for hunk in &mut hunks {
            mark_words(&mut hunk.lines, deadline);
        }
    }
    FileDiff { hunks, whole }
}

/// Numbers each distinct line, so lines compare as numbers.
fn intern<'a>(old: &[&'a str], new: &[&'a str]) -> (Vec<u32>, Vec<u32>) {
    let mut numbers: HashMap<&'a str, u32> = HashMap::new();
    let mut number = |line: &'a str| -> u32 {
        let next = numbers.len() as u32;
        *numbers.entry(line).or_insert(next)
    };
    let a = old.iter().map(|line| number(line)).collect();
    let b = new.iter().map(|line| number(line)).collect();
    (a, b)
}

fn whole_change(old: usize, new: usize) -> Vec<Op> {
    let mut ops = vec![Op::Removed; old];
    ops.extend(std::iter::repeat_n(Op::Added, new));
    ops
}

/// The edits from `a` to `b`, with the common start and end taken off first; `None` past the
/// deadline or the edit limit.
fn edit_script<T: PartialEq>(a: &[T], b: &[T], deadline: &Deadline) -> Option<Vec<Op>> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let (a_rest, b_rest) = (&a[prefix..], &b[prefix..]);
    let suffix = a_rest
        .iter()
        .rev()
        .zip(b_rest.iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let middle_a = &a_rest[..a_rest.len() - suffix];
    let middle_b = &b_rest[..b_rest.len() - suffix];
    let mut ops = vec![Op::Same; prefix];
    ops.extend(myers(middle_a, middle_b, deadline)?);
    ops.extend(std::iter::repeat_n(Op::Same, suffix));
    Some(ops)
}

/// Myers' greedy diff. Keeps each step's furthest points to trace the path back.
fn myers<T: PartialEq>(a: &[T], b: &[T], deadline: &Deadline) -> Option<Vec<Op>> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    if n == 0 || m == 0 {
        return Some(whole_change(a.len(), b.len()));
    }
    let limit = (a.len() + b.len()).min(MOST_EDITS) as isize;
    let offset = limit + 1;
    let mut furthest = vec![0isize; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    for d in 0..=limit {
        if d % STEPS_PER_LOOK == 0 && deadline.passed() {
            return None;
        }
        trace.push(furthest[(offset - d - 1) as usize..=(offset + d + 1) as usize].to_vec());
        if step(&mut furthest, offset, d, a, b) {
            return Some(trace_back(&trace, n, m));
        }
    }
    None
}

/// Whether diagonal `k` at step `d` is reached down from diagonal `k + 1` (an insertion) rather
/// than across from `k - 1` (a deletion), given each diagonal's `furthest` point.
fn came_down(furthest: impl Fn(isize) -> isize, k: isize, d: isize) -> bool {
    k == -d || (k != d && furthest(k - 1) < furthest(k + 1))
}

/// How far `a` from `x` and `b` from `y` run equal.
fn snake<T: PartialEq>(a: &[T], b: &[T], x: isize, y: isize) -> isize {
    let (Ok(x), Ok(y)) = (usize::try_from(x), usize::try_from(y)) else {
        return 0;
    };
    let (Some(a), Some(b)) = (a.get(x..), b.get(y..)) else {
        return 0;
    };
    a.iter()
        .zip(b)
        .take_while(|(one, other)| one == other)
        .count() as isize
}

/// One edit step `d`: moves each diagonal as far as it goes. Returns whether one reached the end.
fn step<T: PartialEq>(furthest: &mut [isize], offset: isize, d: isize, a: &[T], b: &[T]) -> bool {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let at = |k: isize| (offset + k) as usize;
    for k in (-d..=d).step_by(2) {
        let start = if came_down(|k| furthest[at(k)], k, d) {
            furthest[at(k + 1)]
        } else {
            furthest[at(k - 1)] + 1
        };
        let x = start + snake(a, b, start, start - k);
        furthest[at(k)] = x;
        if x >= n && x - k >= m {
            return true;
        }
    }
    false
}

/// Follows the kept steps back from the end to the start.
fn trace_back(trace: &[Vec<isize>], n: isize, m: isize) -> Vec<Op> {
    let (mut x, mut y) = (n, m);
    let mut ops = Vec::new();
    for (d, kept) in trace.iter().enumerate().rev() {
        let d = d as isize;
        let at = |k: isize| kept[(k + d + 1) as usize];
        let k = x - y;
        let down = came_down(at, k, d);
        let before_k = if down { k + 1 } else { k - 1 };
        let before_x = at(before_k);
        let before_y = before_x - before_k;
        // The equal run that followed the step.
        let same = (x - before_x).min(y - before_y).max(0);
        ops.extend(std::iter::repeat_n(Op::Same, same as usize));
        if d > 0 {
            ops.push(if down { Op::Added } else { Op::Removed });
        }
        (x, y) = (before_x, before_y);
    }
    ops.reverse();
    ops
}

/// Each op with its line and the line numbers before it, 0-based.
struct Placed {
    op: Op,
    old: usize,
    new: usize,
}

fn place(ops: &[Op]) -> Vec<Placed> {
    let (mut old, mut new) = (0, 0);
    ops.iter()
        .map(|&op| {
            let placed = Placed { op, old, new };
            match op {
                Op::Same => (old, new) = (old + 1, new + 1),
                Op::Removed => old += 1,
                Op::Added => new += 1,
            }
            placed
        })
        .collect()
}

/// The ranges of `placed` each hunk covers: changes closer than twice the context share one.
fn hunk_ranges(placed: &[Placed]) -> Vec<Range<usize>> {
    let mut ranges: Vec<Range<usize>> = Vec::new();
    let changes = placed
        .iter()
        .enumerate()
        .filter(|(_, placed)| placed.op != Op::Same);
    for (index, _) in changes {
        let start = index.saturating_sub(CONTEXT_LINES);
        let end = (index + 1 + CONTEXT_LINES).min(placed.len());
        match ranges.last_mut() {
            Some(last) if start <= last.end => last.end = end,
            _ => ranges.push(start..end),
        }
    }
    ranges
}

fn build_hunks(ops: &[Op], old: &[&str], new: &[&str]) -> Vec<Hunk> {
    let placed = place(ops);
    hunk_ranges(&placed)
        .into_iter()
        .map(|range| build_hunk(&placed[range], old, new))
        .collect()
}

fn build_hunk(placed: &[Placed], old: &[&str], new: &[&str]) -> Hunk {
    let lines: Vec<Line> = placed
        .iter()
        .map(|placed| {
            let (kind, text, old_line, new_line) = match placed.op {
                Op::Same => (
                    LineKind::Context,
                    old[placed.old],
                    Some(placed.old + 1),
                    Some(placed.new + 1),
                ),
                Op::Removed => (
                    LineKind::Removed,
                    old[placed.old],
                    Some(placed.old + 1),
                    None,
                ),
                Op::Added => (LineKind::Added, new[placed.new], None, Some(placed.new + 1)),
            };
            Line {
                kind,
                old: old_line,
                new: new_line,
                text: text.to_owned(),
                words: Vec::new(),
            }
        })
        .collect();
    let count = |kind: LineKind| {
        lines
            .iter()
            .filter(|line| line.kind == LineKind::Context || line.kind == kind)
            .count()
    };
    let (old_count, new_count) = (count(LineKind::Removed), count(LineKind::Added));
    let first = &placed[0];
    let start = |before: usize, count: usize| if count == 0 { before } else { before + 1 };
    Hunk {
        old_start: start(first.old, old_count),
        old_count,
        new_start: start(first.new, new_count),
        new_count,
        lines,
    }
}

/// Marks the changed words of each removed line paired with an added one: a run of removed
/// lines right before a run of added lines pairs them in order.
fn mark_words(lines: &mut [Line], deadline: &Deadline) {
    let mut index = 0;
    while index < lines.len() {
        let removed = run_of(lines, index, LineKind::Removed);
        let added = run_of(lines, index + removed, LineKind::Added);
        for pair in 0..removed.min(added) {
            let (old, new) = (index + pair, index + removed + pair);
            let (old_words, new_words) =
                changed_words(lines[old].shown(), lines[new].shown(), deadline);
            lines[old].words = old_words;
            lines[new].words = new_words;
        }
        index += (removed + added).max(1);
    }
}

/// How many lines of `kind` follow from `start`.
fn run_of(lines: &[Line], start: usize, kind: LineKind) -> usize {
    lines
        .iter()
        .skip(start)
        .take_while(|line| line.kind == kind)
        .count()
}

/// The byte ranges that differ between two lines, word by word, merged where they touch. Past
/// the deadline the lines are left unmarked.
fn changed_words(
    old: &str,
    new: &str,
    deadline: &Deadline,
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    let (old_tokens, new_tokens) = (tokens(old), tokens(new));
    let old_texts: Vec<&str> = old_tokens.iter().map(|range| &old[range.clone()]).collect();
    let new_texts: Vec<&str> = new_tokens.iter().map(|range| &new[range.clone()]).collect();
    let Some(ops) = edit_script(&old_texts, &new_texts, deadline) else {
        return (Vec::new(), Vec::new());
    };
    let (mut old_marked, mut new_marked) = (Vec::new(), Vec::new());
    let (mut old_at, mut new_at) = (0, 0);
    for op in ops {
        match op {
            Op::Same => (old_at, new_at) = (old_at + 1, new_at + 1),
            Op::Removed => {
                merge_range(&mut old_marked, old_tokens[old_at].clone());
                old_at += 1;
            }
            Op::Added => {
                merge_range(&mut new_marked, new_tokens[new_at].clone());
                new_at += 1;
            }
        }
    }
    (old_marked, new_marked)
}

fn merge_range(ranges: &mut Vec<Range<usize>>, range: Range<usize>) {
    match ranges.last_mut() {
        Some(last) if last.end == range.start => last.end = range.end,
        _ => ranges.push(range),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TokenClass {
    Word,
    Space,
    Mark,
}

fn class_of(character: char) -> TokenClass {
    if character.is_alphanumeric() || character == '_' {
        TokenClass::Word
    } else if character.is_whitespace() {
        TokenClass::Space
    } else {
        TokenClass::Mark
    }
}

/// A line's tokens as byte ranges: runs of word characters, runs of spaces, and each other
/// character alone.
fn tokens(line: &str) -> Vec<Range<usize>> {
    let mut found: Vec<(Range<usize>, TokenClass)> = Vec::new();
    for (at, character) in line.char_indices() {
        let class = class_of(character);
        let end = at + character.len_utf8();
        match found.last_mut() {
            Some((range, last)) if *last == class && class != TokenClass::Mark => range.end = end,
            _ => found.push((at..end, class)),
        }
    }
    found.into_iter().map(|(range, _)| range).collect()
}

// MARK: Rows

/// What a drawn row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cell {
    /// The hunk's header.
    Header,
    /// Line `n` of the hunk.
    Line(usize),
}

/// One drawn row of a unified diff: the hunk it belongs to, and what it shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub hunk: usize,
    pub cell: Cell,
}

/// The rows of `diff`: each hunk's header, then its lines.
pub fn rows(diff: &FileDiff) -> Vec<Row> {
    let mut rows = Vec::new();
    for (hunk, lines) in diff.hunks.iter().enumerate() {
        rows.push(Row {
            hunk,
            cell: Cell::Header,
        });
        rows.extend((0..lines.lines.len()).map(|line| Row {
            hunk,
            cell: Cell::Line(line),
        }));
    }
    rows
}

/// Where a row of `text` wraps to fit `columns` monospace cells, as GitHub's and GitLab's
/// reviews and Zed's Markdown and plain text do, so a diff never scrolls sideways: each visual
/// line's first character, as an index into `text` with every tab expanded to the next stop of
/// `tab` (the text the shell draws). The first is always 0. A line breaks after its last
/// whitespace when it has one, else where it overflows; whitespace may hang past the edge. A
/// wide character (CJK, a full-width form, most emoji) takes two cells.
pub fn wrap(text: &str, tab: usize, columns: usize) -> Vec<usize> {
    let tab = tab.max(1);
    let columns = columns.max(1);
    let mut cells: Vec<(bool, usize)> = Vec::with_capacity(text.len());
    for character in text.chars() {
        if character == '\t' {
            let spaces = tab - cells.len() % tab;
            cells.extend(std::iter::repeat_n((true, 1), spaces));
        } else {
            cells.push((character.is_whitespace(), cell_width(character)));
        }
    }
    let mut starts = vec![0];
    let (mut start, mut used, mut space_end) = (0, 0, None);
    for (index, &(space, wide)) in cells.iter().enumerate() {
        if used + wide > columns && index > start && !space {
            let at = space_end.filter(|end| *end > start).unwrap_or(index);
            starts.push(at);
            start = at;
            used = cells[at..index].iter().map(|cell| cell.1).sum();
            space_end = None;
        }
        used += wide;
        if space {
            space_end = Some(index + 1);
        }
    }
    starts
}

/// How many monospace cells `text` takes, its tabs already expanded: where a stretch after it
/// starts, in figures, by the rule `wrap` wraps by.
pub fn cells(text: &str) -> usize {
    text.chars().map(cell_width).sum()
}

/// How many monospace cells `character` takes: two for East Asian wide and full-width forms
/// and most emoji, else one.
fn cell_width(character: char) -> usize {
    let wide = matches!(character as u32,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3041..=0x33FF
            | 0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xA000..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F64F
            | 0x1F900..=0x1F9FF
            | 0x20000..=0x3FFFD);
    if wide {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::testing::{FixedClock, SteppingClock};

    #[test]
    fn a_long_row_wraps_after_its_last_space_else_where_it_overflows() {
        // Fits: one line.
        assert_eq!(wrap("short", 4, 10), [0]);
        assert_eq!(wrap("", 4, 10), [0]);
        // Breaks after the last space that fits; the space hangs on the line it ends.
        assert_eq!(wrap("one two three", 4, 9), [0, 8]);
        assert_eq!(wrap("one two three four", 4, 9), [0, 8, 14]);
        // No space: breaks where it overflows.
        assert_eq!(wrap("abcdefghij", 4, 4), [0, 4, 8]);
        // A tab counts its expanded spaces, and indices are into the expanded text.
        assert_eq!(wrap("\tabcdef", 4, 6), [0, 4]);
        // Wide characters take two cells.
        assert_eq!(wrap("漢字漢字", 4, 4), [0, 2]);
        // Never zero columns.
        assert_eq!(wrap("ab", 4, 0), [0, 1]);
    }

    fn quick(old: &str, new: &str) -> FileDiff {
        let clock = FixedClock::at(0);
        diff(
            old,
            new,
            &Deadline {
                clock: &clock,
                until_ms: u64::MAX,
            },
        )
    }

    /// A diff as unified text: a header per hunk, then each line marked.
    fn unified(diff: &FileDiff) -> String {
        let mut text = String::new();
        for hunk in &diff.hunks {
            text.push_str(&hunk.header());
            text.push('\n');
            for line in &hunk.lines {
                let mark = match line.kind {
                    LineKind::Context => ' ',
                    LineKind::Added => '+',
                    LineKind::Removed => '-',
                };
                text.push(mark);
                text.push_str(line.shown());
                text.push('\n');
            }
        }
        text
    }

    #[test]
    fn keeping_a_hunk_takes_in_only_its_lines_and_undoing_one_puts_back_only_its() {
        let old: String = (1..=30).map(|n| format!("{n}\n")).collect();
        let new = old
            .replacen("2\n", "two\n", 1)
            .replacen("25\n", "", 1)
            .replace("30\n", "30");
        let diff = quick(&old, &new);
        assert_eq!(diff.hunks.len(), 2);
        // Keeping the first leaves only the second between the texts.
        let kept = keep_hunk(&old, &diff.hunks[0]).expect("keep");
        let after = quick(&kept, &new);
        assert_eq!(after.hunks.len(), 1);
        assert_eq!(after.hunks[0].header(), diff.hunks[1].header());
        // Keeping both makes the old text the new one, a missing last newline too.
        let both = keep_hunk(&kept, &after.hunks[0]).expect("keep");
        assert_eq!(both, new);
        // A text that no longer holds the hunk's lines is not changed.
        assert_eq!(keep_hunk("other\n", &diff.hunks[0]), None);
        // Undoing the second in the new text leaves only the first.
        let undone = undo_hunk(&new, &diff.hunks[1]).expect("undo");
        let left = quick(&old, &undone);
        assert_eq!(left.hunks.len(), 1);
        assert_eq!(left.hunks[0].header(), diff.hunks[0].header());
        // Found even after lines above it moved.
        let moved = format!("zero\n{new}");
        let undone = undo_hunk(&moved, &diff.hunks[0]).expect("undo");
        assert!(undone.starts_with("zero\n1\n2\n3\n"));
        assert_eq!(undo_hunk("gone\n", &diff.hunks[0]), None);
        // An added file's one hunk undone is empty.
        let added = quick("", "a\nb\n");
        assert_eq!(undo_hunk("a\nb\n", &added.hunks[0]).as_deref(), Some(""));
        assert_eq!(keep_hunk("", &added.hunks[0]).as_deref(), Some("a\nb\n"));
    }

    #[test]
    fn empty_files_and_a_missing_last_newline() {
        assert_eq!(quick("", ""), FileDiff::default());
        assert_eq!(unified(&quick("", "a\nb\n")), "@@ -0,0 +1,2 @@\n+a\n+b\n");
        assert_eq!(unified(&quick("a\n", "")), "@@ -1,1 +0,0 @@\n-a\n");
        // Adding the last newline changes the last line.
        assert_eq!(
            unified(&quick("a\nb", "a\nb\n")),
            "@@ -1,2 +1,2 @@\n a\n-b\n+b\n"
        );
    }

    #[test]
    fn hunks_keep_three_lines_of_context_and_far_changes_split() {
        let numbered = |named: &[(u32, &str)]| -> String {
            (1..=20)
                .map(|n| match named.iter().find(|(at, _)| *at == n) {
                    Some((_, name)) => format!("{name}\n"),
                    None => format!("{n}\n"),
                })
                .collect()
        };
        let old = numbered(&[]);
        let new = numbered(&[(2, "two"), (18, "eighteen")]);
        let diff = quick(&old, &new);
        assert_eq!(diff.hunks.len(), 2);
        assert_eq!(
            unified(&diff),
            "@@ -1,5 +1,5 @@\n 1\n-2\n+two\n 3\n 4\n 5\n\
             @@ -15,6 +15,6 @@\n 15\n 16\n 17\n-18\n+eighteen\n 19\n 20\n"
        );
        assert_eq!(diff.counts(), (2, 2));
        // Changes six lines apart share one hunk.
        let near = numbered(&[(2, "two"), (9, "nine")]);
        assert_eq!(quick(&old, &near).hunks.len(), 1);
    }

    #[test]
    fn crlf_lines_compare_with_their_endings_and_show_without() {
        let diff = quick("a\r\nb\r\n", "a\nb\r\n");
        assert_eq!(unified(&diff), "@@ -1,2 +1,2 @@\n-a\n+a\n b\n");
        assert_eq!(diff.hunks[0].lines[0].text, "a\r\n");
    }

    #[test]
    fn changed_words_are_marked_in_paired_lines_by_byte() {
        let diff = quick("let größe = 10;\n", "let größe = 12;\n");
        let lines = &diff.hunks[0].lines;
        assert_eq!(lines[0].words, [Range { start: 14, end: 16 }]);
        assert_eq!(&lines[1].shown()[lines[1].words[0].clone()], "12");
        let renamed = quick("foo(bar, baz)\n", "foo(qux, baz)\nextra\n");
        let lines = &renamed.hunks[0].lines;
        assert_eq!(lines[0].words, [Range { start: 4, end: 7 }]);
        assert_eq!(lines[1].words, [Range { start: 4, end: 7 }]);
        // The added line with no partner is not marked.
        assert!(lines[2].words.is_empty());
    }

    #[test]
    fn a_moved_block_is_a_removal_and_an_addition() {
        let old = "a\nb\nc\nd\ne\nf\n";
        let new = "d\ne\nf\na\nb\nc\n";
        let diff = quick(old, new);
        assert_eq!(diff.counts(), (3, 3));
        assert!(!diff.whole);
    }

    #[test]
    fn a_diff_past_its_deadline_changes_the_whole_file() {
        let old: String = (0..5000).map(|n| format!("old {n}\n")).collect();
        let new: String = (0..5000).map(|n| format!("new {}\n", n * 7)).collect();
        // Each look at the clock moves it 10 ms on.
        let clock = SteppingClock::new(0, 10);
        let diff = diff(
            &old,
            &new,
            &Deadline {
                clock: &clock,
                until_ms: DEADLINE_MS,
            },
        );
        assert!(diff.whole);
        assert_eq!(diff.counts(), (5000, 5000));
        assert!(diff.hunks[0].lines.iter().all(|line| line.words.is_empty()));
    }

    #[test]
    fn a_large_file_with_a_small_change_diffs_quickly() {
        let old: String = (0..200_000).map(|n| format!("line {n}\n")).collect();
        let new = old.replace("line 100000\n", "line 100000 changed\n");
        let diff = quick(&old, &new);
        assert!(!diff.whole);
        assert_eq!(diff.counts(), (1, 1));
        assert_eq!(diff.hunks[0].old_start, 99_998);
    }

    #[test]
    fn each_hunk_draws_its_header_then_its_lines() {
        let diff = quick("a\nb\nc\nd\n", "a\nB\nC\nX\nd\n");
        let cells: Vec<(usize, Cell)> =
            rows(&diff).iter().map(|row| (row.hunk, row.cell)).collect();
        assert_eq!(cells.len(), 1 + 7);
        assert_eq!(cells[0], (0, Cell::Header));
        assert_eq!(cells[7], (0, Cell::Line(6)));
    }

    #[test]
    fn text_takes_a_cell_a_character_and_two_for_a_wide_one() {
        assert_eq!(cells(""), 0);
        assert_eq!(cells("let x"), 5);
        assert_eq!(cells("名前 = 1"), 8);
    }
}
