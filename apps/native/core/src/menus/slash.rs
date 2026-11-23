//! Layout of the page's `/` menu. It is 240 pt wide with a 1 px border and 4 pt padding, and a
//! small muted "Basic blocks" heading over the blocks. Each block is a 32 pt row, 2 pt apart,
//! with its glyph in a 24 pt tile, its name and the Markdown that types it. The menu is at most
//! 320 pt tall and scrolls the rest, keeping the lit row in view the way a browser reveals a
//! focused option. The editor (`page::editor::Slash`) decides which blocks match and which is
//! lit; a shell draws backgrounds and glyphs at these frames.

use crate::board::card::Font;
use crate::dialog::layout::{Line, Measure};
use crate::geometry::Rect;
use crate::glyph::Glyph;
use crate::menus::layout::INSET;
use crate::theme::Token;

pub const WIDTH: f64 = 240.0;
const MOST_HEIGHT: f64 = 320.0;
const ROW: f64 = 32.0;
const GAP: f64 = 2.0;
/// The heading: 4 pt above, a 16 pt line, 2 pt below.
const HEADING: f64 = 4.0 + 16.0 + 2.0;
const HEADING_FONT: Font = Font::sans(11.0, Font::MEDIUM);
const HEADING_LINE: f64 = 16.0;
/// The heading text sits 6 pt in from the rows' edge and is at most the menu's width less 22 pt.
const HEADING_IN: f64 = 6.0;
const HEADING_SHORT: f64 = 22.0;
/// A row's tile: 24 pt square, 6 pt in and 4 pt down, with a 14 pt glyph centred in it.
const TILE: f64 = 24.0;
const TILE_IN: f64 = 6.0;
const TILE_DOWN: f64 = 4.0;
const GLYPH: f64 = 14.0;
/// A row's text: 8 pt down on a 16 pt line, 8 pt after the tile. The Markdown sits 6 pt from the
/// row's end and at least 8 pt after the name.
const WORDS_DOWN: f64 = 8.0;
const WORDS_LINE: f64 = 16.0;
const WORDS_GAP: f64 = 8.0;
const KEYS_END: f64 = 6.0;
const LABEL_FONT: Font = Font::sans(12.0, Font::REGULAR);
const KEYS_FONT: Font = Font::mono(11.0);

/// The unscrolled content height: border, padding, heading, then each row with the gap before it.
fn content_height(count: usize) -> f64 {
    1.0 + 4.0 + HEADING + count as f64 * (GAP + ROW) + 4.0 + 1.0
}

/// How tall the menu of `count` entries is drawn.
pub fn height(count: usize) -> f64 {
    MOST_HEIGHT.min(content_height(count))
}

/// Entry `index`'s row with the menu scrolled `offset` down.
fn row_frame(index: usize, offset: f64) -> Rect {
    Rect::new(
        INSET,
        INSET + HEADING + GAP + index as f64 * (ROW + GAP) - offset,
        WIDTH - 2.0 * INSET,
        ROW,
    )
}

/// The scroll that keeps entry `lit` of `count` in view, from `offset`.
pub fn reveal(offset: f64, lit: usize, count: usize) -> f64 {
    if lit >= count {
        return offset;
    }
    let row = row_frame(lit, offset);
    let bottom = height(count) - INSET;
    if row.y < INSET {
        offset - (INSET - row.y)
    } else if row.max_y() > bottom {
        offset + row.max_y() - bottom
    } else {
        offset
    }
}

/// The scroll offset after the wheel moved `delta` (positive is down) in `unit`
/// (`scroll::route::wheel_scrolled`), clamped to the rows. A menu whose rows all fit never
/// scrolls.
pub fn scroll(offset: f64, delta: f64, unit: u32, count: usize) -> f64 {
    crate::scroll::route::wheel_scrolled(offset, delta, unit, height(count), content_height(count))
}

/// One entry as the menu shows it.
pub struct Entry<'a> {
    pub label: &'a str,
    /// The Markdown a person can type instead; empty for none.
    pub shortcut: &'a str,
    pub icon: Glyph,
    /// Its number for `ffi::page_slash_choose`.
    pub command: usize,
}

/// A row in view.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// Its place in the menu, for `nr_page_slash_light`.
    pub index: usize,
    pub command: usize,
    pub frame: Rect,
    pub lit: bool,
    pub tile: Rect,
    pub icon: Glyph,
    pub glyph: Rect,
    pub label: Line,
    pub shortcut: Option<Line>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    /// A query that matches nothing draws no menu.
    pub empty: bool,
    pub heading: Line,
    /// The rows at least partly in view; a press or the pointer inside one is on its entry.
    pub rows: Vec<Row>,
}

/// The menu headed `heading` listing `entries`, entry `lit` lit, scrolled `offset` down.
pub fn layout(
    heading: &str,
    entries: &[Entry],
    lit: usize,
    offset: f64,
    measure: &mut Measure,
) -> Layout {
    let height = height(entries.len());
    let heading = Line::new(
        heading,
        INSET + HEADING_IN,
        INSET + 4.0 - offset,
        WIDTH - HEADING_SHORT,
        HEADING_LINE,
        HEADING_FONT,
        Token::MutedForeground,
    );
    let rows = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let frame = row_frame(index, offset);
            if frame.max_y() <= 0.0 || frame.y >= height {
                return None;
            }
            let tile = Rect::new(frame.x + TILE_IN, frame.y + TILE_DOWN, TILE, TILE);
            let glyph = Rect::new(
                tile.mid_x() - GLYPH / 2.0,
                tile.mid_y() - GLYPH / 2.0,
                GLYPH,
                GLYPH,
            );
            let mut right = frame.max_x() - KEYS_END;
            let shortcut = (!entry.shortcut.is_empty()).then(|| {
                let wide = measure(entry.shortcut, KEYS_FONT);
                right -= wide;
                let line = Line::new(
                    entry.shortcut,
                    right,
                    frame.y + WORDS_DOWN,
                    wide + 1.0,
                    WORDS_LINE,
                    KEYS_FONT,
                    Token::MutedForeground,
                );
                right -= WORDS_GAP;
                line
            });
            let x = tile.max_x() + WORDS_GAP;
            let label = Line::new(
                entry.label,
                x,
                frame.y + WORDS_DOWN,
                right - x,
                WORDS_LINE,
                LABEL_FONT,
                Token::PopoverForeground,
            );
            Some(Row {
                index,
                command: entry.command,
                frame,
                lit: index == lit,
                tile,
                icon: entry.icon,
                glyph,
                label,
                shortcut,
            })
        })
        .collect();
    Layout {
        width: WIDTH,
        height,
        empty: entries.is_empty(),
        heading,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::route::{WHEEL_CLICKS, WHEEL_POINTS};
    use crate::text::typeset::test_measure::sevens;

    fn entries(count: usize) -> Vec<Entry<'static>> {
        (0..count)
            .map(|command| Entry {
                label: "Heading 1",
                shortcut: if command == 0 { "#" } else { "" },
                icon: Glyph::File,
                command,
            })
            .collect()
    }

    #[test]
    fn the_menu_is_as_tall_as_its_rows_up_to_320() {
        // 1 + 4 + 22 + 3 × 34 + 4 + 1.
        assert_eq!(height(3), 134.0);
        assert_eq!(height(12), 320.0);
        assert_eq!(height(0), 32.0);
    }

    #[test]
    fn each_row_has_its_tile_its_name_and_its_markdown_at_the_end() {
        let placed = layout("Basic blocks", &entries(2), 1, 0.0, &mut sevens);
        assert_eq!(
            (placed.width, placed.height, placed.empty),
            (240.0, 100.0, false)
        );
        assert_eq!(
            placed.heading,
            Line::new(
                "Basic blocks",
                11.0,
                9.0,
                218.0,
                16.0,
                HEADING_FONT,
                Token::MutedForeground
            )
        );
        let first = &placed.rows[0];
        assert_eq!(
            (first.frame, first.lit),
            (Rect::new(5.0, 29.0, 230.0, 32.0), false)
        );
        assert_eq!(
            (first.tile, first.glyph),
            (
                Rect::new(11.0, 33.0, 24.0, 24.0),
                Rect::new(16.0, 38.0, 14.0, 14.0)
            )
        );
        // "#" is 7 wide, 6 from the row's end; the name ends 8 before it.
        assert_eq!(
            first.shortcut,
            Some(Line::new(
                "#",
                222.0,
                37.0,
                8.0,
                16.0,
                KEYS_FONT,
                Token::MutedForeground
            ))
        );
        assert_eq!(
            first.label,
            Line::new(
                "Heading 1",
                43.0,
                37.0,
                171.0,
                16.0,
                LABEL_FONT,
                Token::PopoverForeground
            )
        );
        let second = &placed.rows[1];
        assert_eq!(
            (second.frame.y, second.lit, second.shortcut.is_none()),
            (63.0, true, true)
        );
        assert_eq!(second.label.frame.width, 186.0);
        assert!(layout("Basic blocks", &[], 0, 0.0, &mut sevens).empty);
    }

    #[test]
    fn the_lit_row_is_kept_in_view_and_the_wheel_is_held_to_the_rows() {
        // 12 rows: 1 + 4 + 22 + 12 × 34 + 5 = 440 tall, drawn 320.
        assert_eq!(reveal(0.0, 0, 12), 0.0);
        // Row 9 ends at 29 + 9 × 34 + 32 = 367, past 315.
        assert_eq!(reveal(0.0, 9, 12), 52.0);
        // Back up to row 0 scrolled 52: its top at -23 comes to 5.
        assert_eq!(reveal(52.0, 0, 12), 24.0);
        assert_eq!(scroll(0.0, 30.0, WHEEL_POINTS, 12), 30.0);
        assert_eq!(scroll(100.0, 100.0, WHEEL_POINTS, 12), 120.0);
        assert_eq!(scroll(10.0, -30.0, WHEEL_POINTS, 12), 0.0);
        assert_eq!(
            scroll(0.0, 30.0, WHEEL_POINTS, 3),
            0.0,
            "rows that fit do not scroll"
        );
        // One wheel click scrolls 40 pt.
        assert_eq!(scroll(0.0, 1.0, WHEEL_CLICKS, 12), 40.0);
        assert_eq!(scroll(100.0, 2.0, WHEEL_CLICKS, 12), 120.0);
        // Scrolled, the rows out of view are left out and the heading goes up with the rows.
        let placed = layout("Basic blocks", &entries(12), 9, 52.0, &mut sevens);
        assert_eq!(placed.rows.first().map(|row| row.index), Some(0));
        assert_eq!(placed.rows.last().map(|row| row.index), Some(10));
        assert_eq!(placed.heading.frame.y, -43.0);
        let further = layout("Basic blocks", &entries(12), 11, 120.0, &mut sevens);
        assert_eq!(further.rows.first().map(|row| row.index), Some(2));
    }
}
