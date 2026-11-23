//! Where the Changes tab puts its parts in its frame: the scope picker on the list's first line,
//! the flat file list under it on the left and, on the right, one continuous diff of every listed
//! file, each file a section that starts with a slim line naming it. A file's tools sit at its
//! name line's right end; the line of the file in view stays pinned at the diff's top while its
//! section scrolls under it, until the next file's line pushes it up. A hunk's tools sit in a
//! small strip at the right end of its first row, kept below the pinned line while the hunk shows.
//! Frames are in the tab's points from its top-left; section offsets are points down the diff's
//! content.

use crate::board::Frame;

/// One row of the file list.
pub const LIST_ROW: f64 = 28.0;
/// One diff line: monospace 12 px on 16 px lines, as source files show.
pub const DIFF_LINE: f64 = 16.0;
/// The slim line naming a file at the top of its section.
pub const SEPARATOR: f64 = 32.0;
/// What a file without diff lines shows under its name line: its name and what happened.
pub const BLOCK: f64 = 52.0;
/// Space after a section's last line, before the next file's name line.
pub const SECTION_END: f64 = 16.0;
/// Space around the parts' contents and between controls.
pub const INSET: f64 = 16.0;
pub const GAP: f64 = 8.0;
/// A tool on a name line: a square icon button, centred in the line.
pub const LINE_TOOL: f64 = 24.0;
/// A tool in a hunk's strip: an icon button as tall as a diff line, and the strip's padding
/// either side of its tools.
pub const HUNK_TOOL: f64 = 22.0;
pub const STRIP_PAD: f64 = 2.0;
/// The list takes this share of the width, kept between these widths and never more than
/// half the tab (a side pane takes at most an equal share of its row).
const LIST_SHARE: f64 = 0.3;
const LIST_NARROWEST: f64 = 220.0;
const LIST_WIDEST: f64 = 340.0;

/// No frame: where a part goes when it does not show.
pub const NOWHERE: Frame = Frame {
    x: 0.0,
    y: 0.0,
    width: 0.0,
    height: 0.0,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frames {
    /// The scope picker, the list's first line, which stays as the files scroll under it.
    pub picker: Frame,
    pub list: Frame,
    pub diff: Frame,
}

/// The parts of a tab `width` × `height`.
pub fn frames(width: f64, height: f64) -> Frames {
    let (width, height) = (width.max(0.0), height.max(0.0));
    let list_width = (width * LIST_SHARE)
        .clamp(LIST_NARROWEST, LIST_WIDEST)
        .min(width / 2.0);
    let picker_height = (GAP + LIST_ROW).min(height);
    Frames {
        picker: Frame {
            x: 0.0,
            y: 0.0,
            width: list_width,
            height: picker_height,
        },
        list: Frame {
            x: 0.0,
            y: picker_height,
            width: list_width,
            height: height - picker_height,
        },
        diff: Frame {
            x: list_width,
            y: 0.0,
            width: width - list_width,
            height,
        },
    }
}

/// Where a name line's tools, and a hunk strip's, end on the right.
pub fn tools_end(diff: Frame) -> f64 {
    diff.x + diff.width - GAP
}

/// `count` tools of a name line whose top is `top`, left to right, the last ending at `end`.
pub fn line_tools(top: f64, end: f64, count: usize) -> Vec<Frame> {
    let y = top + (SEPARATOR - LINE_TOOL) / 2.0;
    (0..count)
        .map(|index| Frame {
            x: end - (count - index) as f64 * LINE_TOOL,
            y,
            width: LINE_TOOL,
            height: LINE_TOOL,
        })
        .collect()
}

/// Where the name line ends for its path and ending: before its `count` tools.
pub fn line_text_end(end: f64, count: usize) -> f64 {
    end - count as f64 * LINE_TOOL - GAP
}

/// A hunk's strip on the row whose top is `row` (in the tab's points), holding `count` tools,
/// ending at `end`: the strip, and its tools left to right.
pub fn strip(row: f64, end: f64, count: usize) -> (Frame, Vec<Frame>) {
    let tools: Vec<Frame> = (0..count)
        .map(|index| Frame {
            x: end - STRIP_PAD - (count - index) as f64 * HUNK_TOOL,
            y: row,
            width: HUNK_TOOL,
            height: DIFF_LINE,
        })
        .collect();
    let width = count as f64 * HUNK_TOOL + 2.0 * STRIP_PAD;
    let strip = Frame {
        x: end - width,
        y: row,
        width,
        height: DIFF_LINE,
    };
    (strip, tools)
}

/// The top of the row a hunk's strip sits on, in the tab's points: its first row, from `top`,
/// kept below `floor` (the pinned name line's bottom) while the hunk, ending at `bottom`, still
/// has a row below it.
pub fn strip_row(top: f64, bottom: f64, floor: f64) -> f64 {
    top.max(floor).min(bottom - DIFF_LINE).max(top)
}

/// What a point over the diff is on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit<T> {
    /// Tool `T` of file `usize`'s name line.
    Tool(usize, T),
    /// File `usize`'s name line, off its tools: a click folds it.
    Line(usize),
}

/// What point (`x`, `y`) of the tab is on, over `diff` laid out as `sections` and scrolled to
/// `offset`: the pinned name line, then the line under it. A name line's tools are
/// `tools(file)`, left to right, ending at `tools_end`. Returns the frame of what it is on, for
/// a tooltip.
pub fn hit<T: Copy>(
    sections: &Sections,
    diff: Frame,
    offset: f64,
    (x, y): (f64, f64),
    tools: impl Fn(usize) -> Vec<T>,
) -> Option<(Hit<T>, Frame)> {
    if !inside(&diff, x, y) {
        return None;
    }
    let (file, top) = sections.line_at(offset, y - diff.y)?;
    let top = diff.y + top;
    let shown = tools(file);
    let placed = line_tools(top, tools_end(diff), shown.len());
    let line = Frame {
        x: diff.x,
        y: top,
        width: diff.width,
        height: SEPARATOR,
    };
    let on = shown.iter().zip(placed).find(|(_, at)| inside(at, x, y));
    Some(match on {
        Some((tool, at)) => (Hit::Tool(file, *tool), at),
        None => (Hit::Line(file), line),
    })
}

pub fn inside(frame: &Frame, x: f64, y: f64) -> bool {
    (frame.x..frame.x + frame.width).contains(&x) && (frame.y..frame.y + frame.height).contains(&y)
}

/// One line-number column of a file whose highest line number is `highest`, in a monospace face
/// whose figures are `advance` wide: three figures at least, and 8 pt each side.
pub fn gutter(highest: u32, advance: f64) -> f64 {
    let figures = highest.to_string().len().max(3);
    figures as f64 * advance + 16.0
}

/// How many monospace cells `advance` wide fit in a diff `width` wide, after a file's two
/// number columns of `gutter` and the gap before its text, and `INSET` before the right edge.
pub fn text_columns(width: f64, gutter: f64, advance: f64) -> usize {
    let room = width - 2.0 * gutter - GAP - INSET;
    match advance > 0.0 {
        true => (room / advance).floor().max(1.0) as usize,
        false => usize::MAX,
    }
}

/// A file section's height: its name line, then `rows` visual diff lines (a wrapped row takes
/// one per line), or the block of a file without any, then the space before the next. A collapsed (`folded`) section is only its
/// name line.
pub fn section_height(rows: usize, folded: bool) -> f64 {
    let body = match (folded, rows) {
        (true, _) => return SEPARATOR,
        (false, 0) => BLOCK,
        (false, rows) => rows as f64 * DIFF_LINE,
    };
    SEPARATOR + body + SECTION_END
}

/// How far into its section the file in view stays once its height becomes `height`: as far
/// as before while that still falls inside it, else at its top, so a collapsed file's name
/// line comes to the top rather than off the screen.
pub fn kept_within(within: f64, height: f64) -> f64 {
    match within < height {
        true => within.max(0.0),
        false => 0.0,
    }
}

/// Where each file's section starts down the continuous diff.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sections {
    tops: Vec<f64>,
    length: f64,
}

impl Sections {
    /// No sections.
    pub const fn empty() -> Self {
        Self {
            tops: Vec::new(),
            length: 0.0,
        }
    }

    /// Sections of `heights`, one after another from 0.
    pub fn new(heights: impl IntoIterator<Item = f64>) -> Self {
        let mut length = 0.0;
        let tops = heights
            .into_iter()
            .map(|height| {
                let top = length;
                length += height;
                top
            })
            .collect();
        Self { tops, length }
    }

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tops.is_empty()
    }

    /// Where section `index` starts: the offset that scrolls it to the top.
    pub fn top(&self, index: usize) -> Option<f64> {
        self.tops.get(index).copied()
    }

    pub fn height(&self, index: usize) -> Option<f64> {
        let top = self.top(index)?;
        let end = self.tops.get(index + 1).copied().unwrap_or(self.length);
        Some(end - top)
    }

    /// The file in view at scroll `offset`: the one whose section holds the diff's top edge.
    pub fn in_view(&self, offset: f64) -> Option<usize> {
        let after = self.tops.partition_point(|top| *top <= offset);
        match self.tops.len() {
            0 => None,
            _ => Some(after.saturating_sub(1)),
        }
    }

    /// The file whose name line holds point `y` down the diff, if any.
    pub fn name_line_at(&self, y: f64) -> Option<usize> {
        if !(0.0..self.length).contains(&y) {
            return None;
        }
        let index = self.in_view(y)?;
        (y - self.top(index)? < SEPARATOR).then_some(index)
    }

    /// The name line pinned at the diff's top at scroll `offset`: the file in view's (so the
    /// scroll-spy agrees), and its top down from the diff's top edge. It stays at 0 while its
    /// section scrolls under it, and the next file's line pushes it up and out: it never sits
    /// lower than its own place, and its bottom never below the end of its section. A collapsed
    /// file is only its line, so its pinned line is where the line is.
    pub fn pinned(&self, offset: f64) -> Option<(usize, f64)> {
        let index = self.in_view(offset)?;
        let (top, height) = (self.top(index)?, self.height(index)?);
        let pushed = top + height - offset - SEPARATOR;
        Some((index, pushed.min((top - offset).max(0.0))))
    }

    /// The file whose name line is at `y` down from the diff's top edge at scroll `offset`,
    /// and that line's top there: the pinned line covers the one beneath it.
    pub fn line_at(&self, offset: f64, y: f64) -> Option<(usize, f64)> {
        if let Some((index, top)) = self.pinned(offset) {
            if (top..top + SEPARATOR).contains(&y) {
                return Some((index, top));
            }
        }
        let index = self.name_line_at(offset + y)?;
        Some((index, self.top(index)? - offset))
    }

    /// Where the file in view at `offset` stays once the sections are laid out as `next`:
    /// its index and how far into its section (`kept_within`), so toggling it keeps its name
    /// line where it is on screen.
    pub fn carry(&self, next: &Sections, offset: f64) -> Option<(usize, f64)> {
        let index = self.in_view(offset)?;
        let within = offset - self.top(index)?;
        Some((index, kept_within(within, next.height(index)?)))
    }

    /// The offset `within` points into section `index`, kept inside it.
    pub fn anchor(&self, index: usize, within: f64) -> Option<f64> {
        let (top, height) = (self.top(index)?, self.height(index)?);
        Some(top + within.clamp(0.0, (height - 1.0).max(0.0)))
    }

    /// The diff's scrolling length in a view `shown` tall: every section, and room enough
    /// below for the last to scroll to the top, so any file can be the one in view.
    pub fn extent(&self, shown: f64) -> f64 {
        match self.tops.last() {
            Some(last) => self.length.max(last + shown),
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_takes_a_share_of_the_width_and_the_diff_the_rest() {
        let laid = frames(1200.0, 800.0);
        assert_eq!(laid.list.width, 340.0);
        assert_eq!(laid.list.y + laid.list.height, 800.0);
        assert_eq!(laid.diff.x, 340.0);
        assert_eq!(laid.diff.x + laid.diff.width, 1200.0);
        assert_eq!((laid.diff.y, laid.diff.height), (0.0, 800.0));
    }

    #[test]
    fn a_narrow_tab_gives_the_list_at_most_half() {
        let laid = frames(300.0, 100.0);
        assert_eq!(laid.list.width, 150.0);
        assert_eq!(laid.diff.width, 150.0);
        let tiny = frames(0.0, 10.0);
        assert_eq!(tiny.diff.width, 0.0);
        assert_eq!(tiny.list.width, 0.0);
    }

    #[test]
    fn the_picker_is_the_list_s_first_line_and_the_files_start_under_it() {
        let laid = frames(1200.0, 800.0);
        assert_eq!(laid.picker.y, 0.0);
        assert_eq!(laid.picker.height, GAP + LIST_ROW);
        assert_eq!(laid.picker.width, laid.list.width);
        assert_eq!(laid.list.y, laid.picker.height);
        assert_eq!(laid.list.y + laid.list.height, 800.0);
    }

    #[test]
    fn a_name_line_s_tools_end_at_the_diff_s_right_edge() {
        let diff = frames(1200.0, 800.0).diff;
        let end = tools_end(diff);
        let tools = line_tools(100.0, end, 3);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[2].x + LINE_TOOL, end);
        assert_eq!(tools[1].x + LINE_TOOL, tools[2].x);
        // Centred in the line.
        assert_eq!(tools[0].y, 100.0 + (SEPARATOR - LINE_TOOL) / 2.0);
        assert_eq!(end, 1200.0 - GAP);
        assert_eq!(line_text_end(end, 2), end - 2.0 * LINE_TOOL - GAP);
    }

    #[test]
    fn a_hunk_s_strip_sits_at_its_first_row_s_right_end_below_the_pinned_line() {
        let (strip, tools) = strip(100.0, 1000.0, 2);
        assert_eq!(strip.x + strip.width, 1000.0);
        assert_eq!((strip.y, strip.height), (100.0, DIFF_LINE));
        assert_eq!(tools[1].x + HUNK_TOOL, 1000.0 - STRIP_PAD);
        assert_eq!(tools[0].x + HUNK_TOOL, tools[1].x);
        assert_eq!(super::strip(100.0, 1000.0, 0).1.len(), 0);
        // Its first row in view: there.
        assert_eq!(strip_row(200.0, 400.0, 32.0), 200.0);
        // Scrolled under the pinned line: just below it, until its last row.
        assert_eq!(strip_row(10.0, 400.0, 32.0), 32.0);
        assert_eq!(strip_row(-300.0, 40.0, 32.0), 40.0 - DIFF_LINE);
    }

    #[test]
    fn a_file_s_text_has_the_room_its_numbers_leave() {
        assert_eq!(gutter(7, 7.0), 3.0 * 7.0 + 16.0);
        assert_eq!(gutter(12_345, 7.0), 5.0 * 7.0 + 16.0);
        let width = 800.0;
        let numbers = gutter(10, 7.0);
        let room = width - 2.0 * numbers - GAP - INSET;
        assert_eq!(text_columns(width, numbers, 7.0), (room / 7.0) as usize);
        assert_eq!(text_columns(10.0, numbers, 7.0), 1);
        // Not measured yet: nothing wraps.
        assert_eq!(text_columns(width, numbers, 0.0), usize::MAX);
    }

    #[test]
    fn a_section_is_its_name_line_and_its_rows_or_a_block() {
        assert_eq!(
            section_height(10, false),
            SEPARATOR + 10.0 * DIFF_LINE + SECTION_END
        );
        assert_eq!(section_height(0, false), SEPARATOR + BLOCK + SECTION_END);
        // Collapsed, with lines or without, it is only its name line.
        assert_eq!(section_height(10, true), SEPARATOR);
        assert_eq!(section_height(0, true), SEPARATOR);
    }

    #[test]
    fn the_scroll_spy_and_a_click_count_collapsed_sections_as_their_name_line() {
        // The first file collapsed, the second open with 10 rows, the third collapsed.
        let heights = [(10, true), (10, false), (0, true)];
        let sections = Sections::new(heights.map(|(rows, folded)| section_height(rows, folded)));
        let open = section_height(10, false);
        assert_eq!(sections.top(1), Some(SEPARATOR));
        assert_eq!(sections.top(2), Some(SEPARATOR + open));
        assert_eq!(sections.in_view(SEPARATOR - 1.0), Some(0));
        assert_eq!(sections.in_view(SEPARATOR), Some(1));
        assert_eq!(sections.in_view(SEPARATOR + open), Some(2));
        for index in 0..sections.len() {
            assert_eq!(
                sections.in_view(sections.top(index).expect("top")),
                Some(index)
            );
        }
        // A name line is the first SEPARATOR points of each section; past the end is none.
        assert_eq!(sections.name_line_at(0.0), Some(0));
        assert_eq!(sections.name_line_at(SEPARATOR + 5.0), Some(1));
        assert_eq!(sections.name_line_at(SEPARATOR + 40.0), None);
        assert_eq!(sections.name_line_at(SEPARATOR + open + 1.0), Some(2));
        assert_eq!(sections.name_line_at(SEPARATOR * 2.0 + open), None);
        assert_eq!(sections.name_line_at(-1.0), None);
    }

    #[test]
    fn toggling_the_file_in_view_keeps_its_name_line_where_it_is() {
        let open = section_height(20, false);
        let before = Sections::new([open, open, open]);
        let second_collapsed = Sections::new([open, SEPARATOR, open]);
        // Its name line partly shown at the top: it stays as it was.
        assert_eq!(
            before.carry(&second_collapsed, open + 10.0),
            Some((1, 10.0))
        );
        // Scrolled into its rows, its name line was above the top: it comes to the top.
        assert_eq!(
            before.carry(&second_collapsed, open + 100.0),
            Some((1, 0.0))
        );
        // Expanding keeps the file in view as far in as it was.
        assert_eq!(
            second_collapsed.carry(&before, open + 10.0),
            Some((1, 10.0))
        );
        // A file below the one in view changing leaves the view where it was.
        let third_collapsed = Sections::new([open, open, SEPARATOR]);
        assert_eq!(
            before.carry(&third_collapsed, open + 100.0),
            Some((1, 100.0))
        );
        assert_eq!(Sections::default().carry(&before, 0.0), None);
        assert_eq!(kept_within(5.0, 32.0), 5.0);
        assert_eq!(kept_within(32.0, 32.0), 0.0);
    }

    #[test]
    fn the_file_in_view_is_the_one_whose_section_holds_the_top_edge() {
        let sections = Sections::new([100.0, 50.0, 200.0]);
        assert_eq!(sections.in_view(0.0), Some(0));
        assert_eq!(sections.in_view(99.5), Some(0));
        assert_eq!(sections.in_view(100.0), Some(1));
        assert_eq!(sections.in_view(149.0), Some(1));
        assert_eq!(sections.in_view(150.0), Some(2));
        assert_eq!(sections.in_view(10_000.0), Some(2));
        assert_eq!(Sections::default().in_view(0.0), None);
    }

    #[test]
    fn a_file_scrolls_to_its_section_s_top_and_the_last_can_reach_it() {
        let sections = Sections::new([100.0, 50.0, 200.0]);
        assert_eq!(sections.top(1), Some(100.0));
        assert_eq!(sections.top(3), None);
        assert_eq!(sections.height(2), Some(200.0));
        // Shown 500 tall, the last file's top at 150 still scrolls to the top.
        assert_eq!(sections.extent(500.0), 650.0);
        assert_eq!(sections.extent(100.0), 350.0);
        assert_eq!(Sections::default().extent(500.0), 0.0);
        // Clicking a file scrolls to its top, which puts it in view.
        for index in 0..sections.len() {
            let top = sections.top(index).expect("top");
            assert_eq!(sections.in_view(top), Some(index));
        }
    }

    #[test]
    fn an_anchor_stays_inside_its_section() {
        let sections = Sections::new([100.0, 50.0]);
        assert_eq!(sections.anchor(1, 20.0), Some(120.0));
        assert_eq!(sections.anchor(1, 80.0), Some(149.0));
        assert_eq!(sections.anchor(0, -5.0), Some(0.0));
        assert_eq!(sections.anchor(2, 0.0), None);
    }

    /// Three open files of 20 rows, laid out one after another.
    fn three_open() -> (Sections, f64) {
        let open = section_height(20, false);
        (Sections::new([open, open, open]), open)
    }

    #[test]
    fn the_file_in_view_s_line_stays_pinned_until_the_next_pushes_it_up() {
        let (sections, open) = three_open();
        // At a section's top its line is where it belongs.
        assert_eq!(sections.pinned(0.0), Some((0, 0.0)));
        // Scrolled into its rows, it stays at the top.
        assert_eq!(sections.pinned(100.0), Some((0, 0.0)));
        assert_eq!(sections.pinned(open - SEPARATOR), Some((0, 0.0)));
        // The next line reaches it and pushes it up, point for point.
        assert_eq!(sections.pinned(open - SEPARATOR + 10.0), Some((0, -10.0)));
        assert_eq!(sections.pinned(open - 1.0), Some((0, 1.0 - SEPARATOR)));
        // Pushed out: the next file is in view and pinned at the top.
        assert_eq!(sections.pinned(open), Some((1, 0.0)));
        assert_eq!(sections.pinned(open + 50.0), Some((1, 0.0)));
        // The last file stays pinned in the room below it.
        assert_eq!(
            sections.pinned(2.0 * open + 300.0).map(|(at, _)| at),
            Some(2)
        );
        assert_eq!(Sections::default().pinned(0.0), None);
    }

    #[test]
    fn a_collapsed_file_pins_no_more_than_its_own_line() {
        let open = section_height(20, false);
        let sections = Sections::new([SEPARATOR, open]);
        // Scrolled 10 into a collapsed file: its line is where it is, partly above the top.
        assert_eq!(sections.pinned(10.0), Some((0, -10.0)));
        assert_eq!(sections.pinned(SEPARATOR), Some((1, 0.0)));
    }

    #[test]
    fn the_scroll_spy_lights_the_pinned_file() {
        let (sections, open) = three_open();
        let folded = Sections::new([open, SEPARATOR, open, SEPARATOR]);
        for laid in [&sections, &folded] {
            let mut offset = 0.0;
            while offset < 3.0 * open {
                let pinned = laid.pinned(offset).map(|(at, _)| at);
                assert_eq!(pinned, laid.in_view(offset), "at {offset}");
                offset += 7.0;
            }
        }
    }

    #[test]
    fn the_pinned_line_takes_clicks_over_the_rows_under_it() {
        let (sections, open) = three_open();
        // Scrolled 100 into the first file: the top line is its pinned line.
        assert_eq!(sections.line_at(100.0, 5.0), Some((0, 0.0)));
        assert_eq!(sections.line_at(100.0, SEPARATOR + 5.0), None);
        // While pushed, the pinned line and the next line below it each take their own.
        let pushing = open - SEPARATOR + 10.0;
        assert_eq!(sections.line_at(pushing, 0.0), Some((0, -10.0)));
        assert_eq!(
            sections.line_at(pushing, SEPARATOR - 10.0),
            Some((1, SEPARATOR - 10.0))
        );
        // Not scrolled, each line is where it is.
        assert_eq!(sections.line_at(0.0, open + 3.0), Some((1, open)));
    }

    #[test]
    fn a_click_on_a_line_s_tool_is_that_tool_and_folds_nothing() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum Tool {
            Undo,
            Open,
        }
        let diff = frames(1200.0, 800.0).diff;
        let (sections, open) = three_open();
        let end = tools_end(diff);
        // The first file has Undo (the pointer is over its line) and Open; the others
        // Undo alone.
        let tools = |file: usize| match file {
            0 => vec![Tool::Undo, Tool::Open],
            _ => vec![Tool::Undo],
        };
        let at = |offset: f64, x: f64, y: f64| {
            hit(&sections, diff, offset, (x, y), tools).map(|(hit, _)| hit)
        };
        let middle = |index: usize| end - (index as f64 + 0.5) * LINE_TOOL;
        // Not scrolled: the first line's tools from the right, then the line itself.
        assert_eq!(at(0.0, middle(0), 16.0), Some(Hit::Tool(0, Tool::Open)));
        assert_eq!(at(0.0, middle(1), 16.0), Some(Hit::Tool(0, Tool::Undo)));
        assert_eq!(at(0.0, middle(2), 16.0), Some(Hit::Line(0)));
        assert_eq!(at(0.0, diff.x + 40.0, 16.0), Some(Hit::Line(0)));
        // The second file's line, lower down.
        assert_eq!(
            at(0.0, middle(0), open + 16.0),
            Some(Hit::Tool(1, Tool::Undo))
        );
        assert_eq!(at(0.0, middle(1), open + 16.0), Some(Hit::Line(1)));
        // Scrolled into the first file, its pinned line's tools work at the top.
        assert_eq!(at(100.0, middle(0), 10.0), Some(Hit::Tool(0, Tool::Open)));
        assert_eq!(at(100.0, diff.x + 40.0, 10.0), Some(Hit::Line(0)));
        // A row under the pinned line, or outside the diff, is on nothing.
        assert_eq!(at(100.0, middle(0), 60.0), None);
        assert_eq!(at(0.0, 10.0, 16.0), None);
        // The tool's frame comes back for its tooltip.
        let (_, frame) = hit(&sections, diff, 0.0, (middle(0), 16.0), tools).expect("a tool");
        assert_eq!(frame.x + frame.width, end);
    }
}
