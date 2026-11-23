//! Layout of a drawn menu's rows. The popover surface has a 1 px border and 4 pt padding; rows
//! are 24 pt tall, 2 pt apart, in 12 pt type. The menu is as wide as its widest row (each capped
//! at 256 pt) and at least 112 pt. A shell draws each row's text, glyph, tick, shortcut and
//! submenu caret at these frames, in points from the menu's top-left; text is measured by the
//! core's typesetter (`typeset`).

use crate::board::card::Font;
use crate::board::Frame;

/// What a row is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Does something when chosen, or opens its submenu.
    Item,
    /// Text that cannot be chosen.
    Label,
    Separator,
}

/// A row as the menu lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: Kind,
    pub title: String,
    /// The shortcut as written beside the text; empty for none.
    pub shortcut: String,
    /// A glyph or image goes before the text.
    pub image: bool,
    /// Shows its tick, or a dash.
    pub ticked: bool,
    pub submenu: bool,
    pub enabled: bool,
    /// It follows the row above with no gap.
    pub tight: bool,
    /// Muted text on a second line under an item's own; empty for none.
    pub detail: String,
}

/// How a menu is set beyond its rows (`menus::Menu`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Minimum width; 0 to fit the rows.
    pub min_width: f64,
    /// Maximum height, beyond which the rows scroll; 0 for no limit.
    pub max_height: f64,
    /// The size of the glyphs before the text, and the gap from a glyph to its text.
    pub icon_size: f64,
    pub icon_gap: f64,
    /// Labels are small and muted headings over the rows under them, and shortcuts are set in
    /// monospace, as in the page's block menu.
    pub quiet_labels: bool,
}

/// The default style: 14 pt glyphs, 6 pt from their text.
pub const PLAIN: Style = Style {
    min_width: 0.0,
    max_height: 0.0,
    icon_size: 14.0,
    icon_gap: 6.0,
    quiet_labels: false,
};

/// Where one row's parts go. A part the row lacks is an empty frame at its row's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub frame: Frame,
    /// The pointer highlights it and the keys reach it.
    pub selectable: bool,
    /// The tick or dash, 12 pt square.
    pub check: Frame,
    /// The glyph or image before the text.
    pub icon: Frame,
    /// A submenu's caret.
    pub caret: Frame,
    /// The text's line box: its top, height and maximum width.
    pub words: Frame,
    pub words_font: Font,
    /// The shortcut's line box, with text starting at its left; a value row's value is aligned to
    /// its right edge instead.
    pub keys: Frame,
    pub keys_font: Font,
    /// The line box of an item's second line, as wide as the row inside its padding.
    pub detail: Frame,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    /// Drawn height, and the full height of the rows.
    pub height: f64,
    pub content_height: f64,
    pub places: Vec<Place>,
}

const FONT: Font = Font::sans(12.0, Font::REGULAR);
const LABEL_FONT: Font = Font::sans(12.0, Font::MEDIUM);
const QUIET_LABEL_FONT: Font = Font::sans(11.0, Font::MEDIUM);
const KEYS_FONT: Font = Font::sans(11.0, Font::REGULAR);
const QUIET_KEYS_FONT: Font = Font::mono(11.0);
/// An item's second line.
pub const DETAIL_FONT: Font = Font::sans(11.0, Font::REGULAR);

const MIN_WIDTH: f64 = 112.0;
/// The most a row's content adds to the menu's width.
const CONTENT_CAP: f64 = 256.0;
/// The 1 px border plus 4 pt padding, shared with the slash menu.
pub(crate) const INSET: f64 = 5.0;
const ROW: f64 = 24.0;
/// An item with a second line: its first line where a normal row's is, the second 14 pt tall
/// under it.
const DETAIL_ROW: f64 = 40.0;
const DETAIL_TOP: f64 = 21.0;
const DETAIL_LINE: f64 = 14.0;
const GAP: f64 = 2.0;
/// Text sits 6 pt in from a row's edge; the tick's slot and the room it takes.
const ROW_PADDING: f64 = 6.0;
const CHECK: f64 = 12.0;
const CHECK_SLOT: f64 = 22.0;
/// A submenu caret's size, and the gap before it.
const CARET: f64 = 12.0;
const CARET_GAP: f64 = 6.0;
/// A shortcut sits at the row's far edge, at least 8 pt from the text when measuring width and
/// 12 pt when placed.
const KEYS_GAP: f64 = 8.0;
const KEYS_ROOM: f64 = 12.0;

fn rect(x: f64, y: f64, width: f64, height: f64) -> Frame {
    Frame {
        x,
        y,
        width,
        height,
    }
}

/// Lays out `rows` in `style`, with `measure` giving the width of text in a font.
pub fn layout(rows: &[Row], style: &Style, measure: &mut dyn FnMut(&str, Font) -> f64) -> Layout {
    // If any row is ticked, every item reserves room for the tick.
    let checkable = rows.iter().any(|row| row.ticked);
    let tick_room = if checkable { CHECK_SLOT } else { 0.0 };
    let keys_font = style.keys_font();
    let content = rows
        .iter()
        .filter(|row| row.kind != Kind::Separator)
        .map(|row| content_width(row, tick_room, style, keys_font, measure).min(CONTENT_CAP))
        .fold(0.0, f64::max);
    // Round to the device pixel, as a browser paints a box of that width.
    let width = MIN_WIDTH
        .max(style.min_width)
        .max(((content + 2.0 * ROW_PADDING + 2.0 * (INSET - 1.0) + 2.0) * 2.0).round() / 2.0);
    let mut y = INSET;
    let mut places = Vec::with_capacity(rows.len());
    for row in rows {
        if row.tight {
            y -= GAP;
        }
        let height = row.height();
        let frame = match row.kind {
            // 1 px, with the row gap above and below, extending 2 pt past the rows on each side.
            Kind::Separator => rect(INSET - 2.0, y, width - 2.0 * (INSET - 2.0), 1.0),
            _ => rect(INSET, y, width - 2.0 * INSET, height),
        };
        places.push(place(row, frame, checkable, style, keys_font, measure));
        y += height + GAP;
    }
    let content_height = y - GAP + INSET;
    let height = if style.max_height > 0.0 {
        content_height.min(style.max_height)
    } else {
        content_height
    };
    Layout {
        width,
        height,
        content_height,
        places,
    }
}

impl Style {
    /// The shortcut font: monospace when labels are muted.
    fn keys_font(&self) -> Font {
        if self.quiet_labels {
            QUIET_KEYS_FONT
        } else {
            KEYS_FONT
        }
    }
}

impl Row {
    fn has_detail(&self) -> bool {
        self.kind == Kind::Item && !self.detail.is_empty()
    }

    fn height(&self) -> f64 {
        match self.kind {
            Kind::Separator => 1.0,
            Kind::Item if self.has_detail() => DETAIL_ROW,
            Kind::Item | Kind::Label => ROW,
        }
    }
}

/// A row's content width: its text plus the room for its image, tick (`tick_room` for an
/// item), shortcut and caret.
fn content_width(
    row: &Row,
    tick_room: f64,
    style: &Style,
    keys_font: Font,
    measure: &mut dyn FnMut(&str, Font) -> f64,
) -> f64 {
    let item = row.kind == Kind::Item;
    let mut width = measure(&row.title, if item { FONT } else { LABEL_FONT });
    if row.image {
        width += style.icon_size + style.icon_gap;
    }
    if item {
        width += tick_room;
    }
    if !row.shortcut.is_empty() {
        width += KEYS_GAP + measure(&row.shortcut, keys_font);
    }
    if row.submenu {
        width += CARET_GAP + CARET;
    }
    if row.has_detail() {
        let before = if row.image {
            style.icon_size + style.icon_gap
        } else {
            0.0
        };
        width = width.max(tick_room + before + measure(&row.detail, DETAIL_FONT));
    }
    width
}

fn place(
    row: &Row,
    frame: Frame,
    checkable: bool,
    style: &Style,
    keys_font: Font,
    measure: &mut dyn FnMut(&str, Font) -> f64,
) -> Place {
    let corner = rect(frame.x, frame.y, 0.0, 0.0);
    let mut place = Place {
        frame,
        selectable: row.kind == Kind::Item && row.enabled,
        check: corner,
        icon: corner,
        caret: corner,
        words: corner,
        words_font: FONT,
        keys: corner,
        keys_font,
        detail: corner,
    };
    let line = |x: f64, top: f64, width: f64, height: f64| rect(x, frame.y + top, width, height);
    let mut x = frame.x + ROW_PADDING;
    // Only an item has a tick.
    if checkable && row.kind == Kind::Item {
        place.check = rect(x, frame.y + 6.0, CHECK, CHECK);
        x += CHECK_SLOT;
    }
    match row.kind {
        Kind::Separator => {}
        Kind::Item | Kind::Label => {
            if row.image {
                // Beside the row's own line, whether or not a second one follows it.
                place.icon = rect(
                    x,
                    frame.y + (ROW - style.icon_size) / 2.0,
                    style.icon_size,
                    style.icon_size,
                );
                x += style.icon_size + style.icon_gap;
            }
            if row.has_detail() {
                let room = frame.x + frame.width - ROW_PADDING - x;
                place.detail = line(x, DETAIL_TOP, room.max(0.0), DETAIL_LINE);
            }
            let trailing = place.put_caret_and_keys(row, measure);
            let room = (trailing - x).max(0.0) + 0.5;
            (place.words, place.words_font) = match row.kind {
                Kind::Label if style.quiet_labels => (line(x, 6.0, room, 16.0), QUIET_LABEL_FONT),
                Kind::Label => (line(x, 4.0, room, 16.0), LABEL_FONT),
                _ => (line(x, 4.0, room, 16.0), FONT),
            };
        }
    }
    place
}

impl Place {
    /// Places the submenu caret and the shortcut, working in from the row's far edge. Returns where
    /// the space left for the text ends.
    fn put_caret_and_keys(&mut self, row: &Row, measure: &mut dyn FnMut(&str, Font) -> f64) -> f64 {
        let frame = self.frame;
        let mut trailing = frame.x + frame.width - ROW_PADDING;
        if row.submenu {
            self.caret = rect(trailing - CARET, frame.y + 6.0, CARET, CARET);
            trailing -= CARET + CARET_GAP;
        }
        if !row.shortcut.is_empty() {
            let width = measure(&row.shortcut, self.keys_font);
            self.keys = rect(trailing - width, frame.y + 4.0, width + 1.0, 16.0);
            trailing -= width + KEYS_ROOM;
        }
        trailing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Measures every character as 6 pt wide in any font.
    fn sixes(text: &str, _: Font) -> f64 {
        text.chars().count() as f64 * 6.0
    }

    fn row(kind: Kind, title: &str) -> Row {
        Row {
            kind,
            title: title.to_owned(),
            shortcut: String::new(),
            image: false,
            ticked: false,
            submenu: false,
            enabled: true,
            tight: false,
            detail: String::new(),
        }
    }

    #[test]
    fn an_item_with_a_second_line_is_taller_and_as_wide_as_that_line() {
        let mut session = row(Kind::Item, "Outreach");
        session.image = true;
        session.detail = "Sales / Q4 leads · Drafting emails".into();
        let placed = layout(
            &[
                row(Kind::Label, "Needs you"),
                session,
                row(Kind::Item, "Next"),
            ],
            &PLAIN,
            &mut sixes,
        );
        // 34 letters of second line after the glyph's 20, and 22 around the row.
        assert_eq!(placed.width, 34.0 * 6.0 + 20.0 + 22.0);
        let tops: Vec<f64> = placed.places.iter().map(|place| place.frame.y).collect();
        assert_eq!(tops, [5.0, 31.0, 73.0], "40 tall, then the gap of 2");
        let second = placed.places[1];
        assert_eq!(second.frame.height, 40.0);
        assert_eq!(
            second.icon,
            rect(11.0, 36.0, 14.0, 14.0),
            "beside the first line"
        );
        assert_eq!(second.words.y, 35.0);
        assert_eq!(
            second.detail,
            rect(31.0, 52.0, placed.width - 11.0 - 31.0, 14.0)
        );
        // A label's second line is not drawn, and a row without one has no box for it.
        let mut label = row(Kind::Label, "Idle");
        label.detail = "ignored".into();
        let plain = layout(&[label], &PLAIN, &mut sixes);
        assert_eq!(plain.places[0].frame.height, 24.0);
        assert_eq!(plain.places[0].detail, rect(5.0, 5.0, 0.0, 0.0));
    }

    #[test]
    fn a_menu_is_as_wide_as_its_widest_row_and_never_under_112() {
        let short = layout(&[row(Kind::Item, "Open")], &PLAIN, &mut sixes);
        assert_eq!((short.width, short.height), (112.0, 5.0 + 24.0 + 5.0));
        let long = layout(&[row(Kind::Item, &"x".repeat(20))], &PLAIN, &mut sixes);
        assert_eq!(long.width, 120.0 + 22.0);
        // A row counts as at most 256 pt wide.
        let capped = layout(&[row(Kind::Item, &"x".repeat(100))], &PLAIN, &mut sixes);
        assert_eq!(capped.width, 256.0 + 22.0);
        let wide = layout(
            &[row(Kind::Item, "Open")],
            &Style {
                min_width: 224.0,
                ..PLAIN
            },
            &mut sixes,
        );
        assert_eq!(wide.width, 224.0);
    }

    #[test]
    fn the_glyph_tick_shortcut_and_caret_each_take_their_room() {
        let mut item = row(Kind::Item, "Rename");
        item.image = true;
        item.shortcut = "⌘R".into();
        item.submenu = true;
        let mut ticked = row(Kind::Item, "Tagged");
        ticked.ticked = true;
        let placed = layout(&[item, ticked], &PLAIN, &mut sixes);
        // 36 of words, 20 of glyph, 22 of tick, 8 + 12 of shortcut, 18 of caret.
        assert_eq!(placed.width, 36.0 + 20.0 + 22.0 + 20.0 + 18.0 + 22.0);
        let first = placed.places[0];
        assert_eq!(first.check, rect(11.0, 11.0, 12.0, 12.0));
        assert_eq!(first.icon, rect(33.0, 10.0, 14.0, 14.0));
        assert_eq!(
            first.caret,
            rect(placed.width - 5.0 - 6.0 - 12.0, 11.0, 12.0, 12.0)
        );
        assert_eq!(
            first.keys,
            rect(placed.width - 5.0 - 6.0 - 18.0 - 12.0, 9.0, 13.0, 16.0)
        );
        assert_eq!(first.words.x, 53.0);
        assert_eq!(first.words.width, first.keys.x - 12.0 - 53.0 + 0.5);
    }

    #[test]
    fn rows_stack_two_apart_and_a_tight_row_closes_the_gap() {
        let mut tight = row(Kind::Item, "Billing");
        tight.tight = true;
        let rows = [
            row(Kind::Label, "Ada"),
            row(Kind::Separator, ""),
            row(Kind::Item, "Account"),
            tight,
        ];
        let placed = layout(&rows, &PLAIN, &mut sixes);
        let tops: Vec<f64> = placed.places.iter().map(|place| place.frame.y).collect();
        assert_eq!(tops, [5.0, 31.0, 34.0, 58.0]);
        assert_eq!(placed.places[1].frame, rect(3.0, 31.0, 112.0 - 6.0, 1.0));
        assert_eq!(placed.content_height, 58.0 + 24.0 + 5.0);
        let selectable: Vec<bool> = placed.places.iter().map(|place| place.selectable).collect();
        assert_eq!(selectable, [false, false, true, true]);
    }

    #[test]
    fn a_tall_menu_is_cut_to_its_most_and_scrolls() {
        let rows: Vec<Row> = (0..20).map(|_| row(Kind::Item, "Row")).collect();
        let placed = layout(
            &rows,
            &Style {
                max_height: 200.0,
                ..PLAIN
            },
            &mut sixes,
        );
        assert_eq!(
            (placed.height, placed.content_height),
            (200.0, 5.0 + 20.0 * 26.0 - 2.0 + 5.0)
        );
    }

    #[test]
    fn quiet_labels_are_small_and_shortcuts_monospaced() {
        let mut item = row(Kind::Item, "Text");
        item.shortcut = "/".into();
        let placed = layout(
            &[row(Kind::Label, "Turn into"), item],
            &Style {
                quiet_labels: true,
                ..PLAIN
            },
            &mut sixes,
        );
        assert_eq!(
            (placed.places[0].words.y, placed.places[0].words_font),
            (11.0, QUIET_LABEL_FONT)
        );
        assert_eq!(placed.places[1].keys_font, QUIET_KEYS_FONT);
    }
}
