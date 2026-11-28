//! A glance at a session from the sessions menu: while its row is highlighted, the last lines
//! its terminal shows, on a small panel beside the menu. The panel takes no pointer or keys. A
//! shell draws the lines in `FONT` at `line_frame`, on the menu's own surface at `place`.

use crate::board::card::Font;
use crate::board::Frame;
use crate::remote::screen::Run;
use crate::Relock;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// The most lines a glance shows.
pub const LINES: usize = 8;
/// The terminal's monospace face, at the size of a menu's shortcuts.
pub const FONT: Font = Font::mono(11.0);
/// The panel's width.
const WIDTH: f64 = 320.0;
/// From the panel's edge to its text: a menu's 1 px border and 4 pt padding, then a row's 6 pt.
const PADDING: f64 = 11.0;
const LINE: f64 = 16.0;
/// Between the panel and the menu, and the least between the panel and the window's edge.
const GAP: f64 = 4.0;

/// The text of one screen line: its runs at their columns, with spaces where it shows nothing.
fn text_of(row: &[Run]) -> String {
    let mut text = String::new();
    for run in row {
        let behind = run.column.saturating_sub(text.width());
        text.push_str(&" ".repeat(behind));
        text.push_str(&run.text);
    }
    text
}

/// `line` cut to `columns` terminal columns, ending in "…" when cut.
fn cut(line: &str, columns: usize) -> String {
    if line.width() <= columns {
        return line.to_owned();
    }
    let mut kept = String::new();
    let mut taken = 0;
    for letter in line.chars() {
        taken += letter.width().unwrap_or(0);
        // The last column is the ellipsis'.
        if taken >= columns {
            break;
        }
        kept.push(letter);
    }
    format!("{}…", kept.trim_end())
}

/// The last `LINES` of `rows` that show something, top to bottom, each cut to `columns`.
pub fn last_lines(rows: &[String], columns: usize) -> Vec<String> {
    let mut lines: Vec<String> = rows
        .iter()
        .rev()
        .filter(|row| !row.trim().is_empty())
        .take(LINES)
        .map(|row| cut(row, columns))
        .collect();
    lines.reverse();
    lines
}

/// How many columns of text `advance` wide fit the panel.
fn columns_for(advance: f64) -> usize {
    ((WIDTH - 2.0 * PADDING) / advance).floor() as usize
}

/// The glance at terminal file `file`'s session: the last lines its screen shows now. Empty
/// when no session runs there or its screen is blank.
pub fn shown(file: &str) -> Vec<String> {
    let screen = crate::app()
        .sessions
        .relock()
        .get(file)
        .and_then(crate::terminal::Terminal::remote_screen);
    let rows: Vec<String> = screen
        .iter()
        .flat_map(|screen| &screen.rows)
        .map(|row| text_of(row))
        .collect();
    last_lines(&rows, columns_for(crate::text::typeset::width("0", FONT)))
}

/// Where a glance of `lines` lines goes in a window `window` wide and tall, for a row whose top
/// is at `row_top` in the menu at `menu` (points from the window's top-left): left of the menu,
/// or right of it when the left has no room, its top level with the row's and moved up to stay
/// inside the window. `None` when neither side has room, or there are no lines.
pub fn place(lines: usize, menu: Frame, row_top: f64, window: (f64, f64)) -> Option<Frame> {
    if lines == 0 {
        return None;
    }
    let height = 2.0 * PADDING + lines as f64 * LINE;
    let left = menu.x - GAP - WIDTH;
    let right = menu.x + menu.width + GAP;
    let x = if left >= GAP {
        left
    } else if right + WIDTH + GAP <= window.0 {
        right
    } else {
        return None;
    };
    let lowest = (window.1 - GAP - height).max(GAP);
    Some(Frame {
        x,
        y: row_top.clamp(GAP, lowest),
        width: WIDTH,
        height,
    })
}

/// The line box of line `index` on a panel at `panel`: its top, height and the most its text
/// may take.
pub fn line_frame(panel: Frame, index: usize) -> Frame {
    Frame {
        x: panel.x + PADDING,
        y: panel.y + PADDING + index as f64 * LINE,
        width: panel.width - 2.0 * PADDING,
        height: LINE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn a_glance_is_the_last_lines_that_show_something() {
        let screen = rows(&["$ make", "", "cc main.c", "   ", "  done", "", ""]);
        assert_eq!(last_lines(&screen, 40), ["$ make", "cc main.c", "  done"]);
        let long: Vec<String> = (1..=12).map(|line| format!("line {line}")).collect();
        let last = last_lines(&long, 40);
        assert_eq!(last.len(), LINES);
        assert_eq!(
            (last[0].as_str(), last[7].as_str()),
            ("line 5", "line 12"),
            "the newest at the bottom"
        );
        assert_eq!(last_lines(&rows(&["", "  "]), 40), Vec::<String>::new());
        assert_eq!(last_lines(&[], 40), Vec::<String>::new());
    }

    #[test]
    fn a_line_wider_than_the_panel_is_cut_with_an_ellipsis() {
        assert_eq!(cut("0123456789", 10), "0123456789");
        assert_eq!(cut("0123456789a", 10), "012345678…");
        // The words before the cut lose the space they would end in.
        assert_eq!(cut("Fix the login form", 9), "Fix the…");
        // A wide character takes two columns.
        assert_eq!(cut("日本語のテキスト", 8), "日本語…");
        assert_eq!(cut("日本語", 6), "日本語");
        assert_eq!(last_lines(&rows(&["abcdefgh", "ok"]), 4), ["abc…", "ok"]);
        // Geist Mono's 6.6 pt advance at 11 pt.
        assert_eq!(columns_for(6.6), 45);
    }

    #[test]
    fn a_screen_line_is_its_runs_at_their_columns() {
        let run = |column: usize, text: &str| Run {
            column,
            text: text.to_owned(),
            ..Run::default()
        };
        assert_eq!(text_of(&[run(0, "$ ls")]), "$ ls");
        assert_eq!(text_of(&[run(2, "a"), run(6, "b")]), "  a   b");
        assert_eq!(text_of(&[run(0, "日本"), run(6, "x")]), "日本  x");
        assert_eq!(text_of(&[]), "");
    }

    #[test]
    fn a_glance_sits_beside_the_menu_level_with_its_row_and_inside_the_window() {
        let menu = Frame {
            x: 600.0,
            y: 46.0,
            width: 300.0,
            height: 200.0,
        };
        let window = (1000.0, 700.0);
        let placed = place(8, menu, 120.0, window).expect("room on the left");
        assert_eq!(
            (placed.x, placed.y, placed.width, placed.height),
            (276.0, 120.0, 320.0, 150.0)
        );
        // Fewer lines make a shorter panel.
        assert_eq!(
            place(2, menu, 120.0, window).map(|at| at.height),
            Some(54.0)
        );
        // A row near the window's bottom: the panel moves up to stay inside.
        assert_eq!(place(8, menu, 650.0, window).map(|at| at.y), Some(546.0));
        // A row scrolled above the window's top.
        assert_eq!(place(8, menu, -30.0, window).map(|at| at.y), Some(4.0));
        // No room on the left: it goes to the right.
        let near = Frame { x: 100.0, ..menu };
        assert_eq!(place(8, near, 120.0, window).map(|at| at.x), Some(404.0));
        // No room on either side, or nothing to show.
        assert_eq!(place(8, near, 120.0, (500.0, 700.0)), None);
        assert_eq!(place(0, menu, 120.0, window), None);
        let line = line_frame(placed, 2);
        assert_eq!(
            (line.x, line.y, line.width, line.height),
            (287.0, 163.0, 298.0, 16.0)
        );
    }
}
