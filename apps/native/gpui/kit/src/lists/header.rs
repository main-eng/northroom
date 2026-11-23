//! The list's header. The core lays out the columns (`nr_list_header_lay_out`); the sorted
//! column shows in the text colour with a caret, Name is followed by the count and the options
//! button, and a hairline marks each resizable edge.
//!
//! The core decides what a press does (`nr_list_header_press`): sort, reset a width, open the
//! options, or start a drag (`nr_list_header_drag`, `nr_list_header_release`) that resizes or
//! moves a column. A right click picks the column for the menu's Move items
//! (`nr_list_header_menu_column`).

use crate::core::{frame, inset, offset, rect, text, zeroed};
use crate::draw::fill;
use crate::icons::{self, Glyph};
use crate::text::{draw_line, width};
use crate::theme::{self, Face, Token, color};
use gpui::{App, Bounds, Pixels, Point, Window};
use northroom_core::ffi::{NrListSort, nr_list_sort};
use northroom_core::menus::ffi::{
    NrHeaderPress, NrListPlaced, nr_list_header_column, nr_list_header_drag,
    nr_list_header_lay_out, nr_list_header_menu_column, nr_list_header_press,
    nr_list_header_release,
};

/// A non-Name column, placed by the core.
#[derive(Clone, Debug, PartialEq)]
pub struct Other {
    pub key: u32,
    pub label: String,
    pub x: f32,
    pub width: f32,
    /// The middle of the gap before it, which a drag resizes it by.
    pub edge: f32,
    pub right_aligned: bool,
}

/// Where the list's columns sit across a row `width` wide.
#[derive(Clone, Debug, PartialEq)]
pub struct Columns {
    pub name_x: f32,
    pub name_width: f32,
    pub name_label: String,
    pub others: Vec<Other>,
    /// The button at the end of Name's column that opens the list's options.
    pub chooser: Bounds<Pixels>,
    /// How far either side of an edge a press counts as a resize.
    pub edge_reach: f32,
}

impl Columns {
    pub fn lay_out(width: f32) -> Self {
        let laid = nr_list_header_lay_out(f64::from(width));
        let others = (0..laid.count)
            .filter_map(|index| {
                let mut placed: NrListPlaced = zeroed();
                // SAFETY: a column to fill.
                unsafe { nr_list_header_column(index, &mut placed) }.then(|| Other {
                    key: placed.key,
                    label: text(&placed.label),
                    x: placed.x as f32,
                    width: placed.width as f32,
                    edge: placed.edge as f32,
                    right_aligned: placed.right_aligned,
                })
            })
            .collect();
        Self {
            name_x: laid.name_x as f32,
            name_width: laid.name_width as f32,
            name_label: text(&laid.name_label),
            others,
            chooser: rect(&laid.chooser),
            edge_reach: laid.edge_reach as f32,
        }
    }

    /// Whether `x` (from the header's left) is on a resizable edge.
    pub fn on_edge(&self, x: f32) -> bool {
        self.others
            .iter()
            .any(|other| (x - other.edge).abs() <= self.edge_reach)
    }
}

/// What a press on the header asks of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pressed {
    Nothing,
    /// The columns, their widths or the sort changed.
    Changed,
    /// The options open under the button.
    Options,
    /// Follow the press until release (`drag`, `release`).
    Follow,
}

/// Handles a press at `at` (from the header's top-left) on a header `width` wide.
pub fn press(width: f32, at: Point<Pixels>, clicks: usize) -> Pressed {
    match nr_list_header_press(
        f64::from(width),
        f64::from(at.x),
        f64::from(at.y),
        clicks as u32,
    ) {
        NrHeaderPress::Nothing => Pressed::Nothing,
        NrHeaderPress::Changed => Pressed::Changed,
        NrHeaderPress::Options => Pressed::Options,
        NrHeaderPress::Follow => Pressed::Follow,
    }
}

/// Moves the followed press to `x`. True when a column's width changed.
pub fn drag(x: f32) -> bool {
    nr_list_header_drag(f64::from(x))
}

/// Releases the followed press at `x`. True when the list changed.
pub fn release(x: f32) -> bool {
    nr_list_header_release(f64::from(x))
}

/// The column whose Move items a right click at `x` shows, or -1 for none.
pub fn menu_column(width: f32, x: f32) -> i64 {
    nr_list_header_menu_column(f64::from(width), f64::from(x))
}

/// A column's header label: its text, position and width from the header's left, its sort
/// key, and whether it is right-aligned.
struct Label {
    text: String,
    x: f32,
    width: f32,
    key: i32,
    right_aligned: bool,
}

impl Label {
    /// Draws the label from the header's top-left (`x0`, `y0`). The sort column uses the text
    /// colour and a direction caret; others are muted.
    fn paint(&self, window: &mut Window, cx: &mut App, sort: &NrListSort, (x0, y0): (f32, f32)) {
        let font = Face::sans(12.0, 400.0);
        let active = sort.key == self.key;
        let (shade, caret) = if active {
            (color(Token::Foreground), 16.0)
        } else {
            (color(Token::MutedForeground), 0.0)
        };
        let label_width = width(&self.text, font).min(self.width - caret);
        let at = if self.right_aligned {
            self.x + self.width - label_width - caret
        } else {
            self.x
        };
        draw_line(
            window,
            cx,
            &self.text,
            font,
            shade,
            x0 + at,
            y0 + 6.0,
            16.0,
            (self.width - caret).max(0.0),
            None,
        );
        if active {
            let glyph = if sort.ascending {
                Glyph::CaretUp
            } else {
                Glyph::CaretDown
            };
            icons::draw(
                window,
                cx,
                glyph,
                frame(x0 + at + label_width + 4.0, y0 + 8.0, 12.0, 12.0),
                shade,
            );
        }
    }
}

/// Draws the header in `header` (window points); `count` is what it says after Name.
pub fn paint(
    window: &mut Window,
    cx: &mut App,
    header: Bounds<Pixels>,
    laid: &Columns,
    count: &str,
) {
    let (x0, y0) = (f32::from(header.origin.x), f32::from(header.origin.y));
    let height = f32::from(header.size.height);
    let muted = color(Token::MutedForeground);
    // After Name: the count, small and muted, then the column chooser.
    let button = offset(laid.chooser, x0, y0);
    icons::draw(
        window,
        cx,
        Glyph::SlidersHorizontal,
        inset(button, 3.0, 3.0),
        muted,
    );
    let small = Face::sans(10.0, 400.0).tabular();
    let counted = width(count, small);
    draw_line(
        window,
        cx,
        count,
        small,
        theme::faded(muted, 0.65),
        f32::from(button.origin.x) - 8.0 - counted,
        y0 + 7.0,
        13.33,
        counted + 1.0,
        None,
    );
    // A hairline in the gap before each other column marks its resize edge.
    for column in &laid.others {
        fill(
            window,
            frame(x0 + column.edge - 0.5, y0 + 6.0, 1.0, 15.0),
            color(Token::Border),
        );
    }
    let sort = nr_list_sort();
    let mut labels = vec![Label {
        text: laid.name_label.clone(),
        x: laid.name_x + 22.0,
        width: laid.name_width,
        key: 0,
        right_aligned: false,
    }];
    labels.extend(laid.others.iter().map(|other| Label {
        text: other.label.clone(),
        x: other.x,
        width: other.width,
        key: other.key as i32,
        right_aligned: other.right_aligned,
    }));
    for label in &labels {
        label.paint(window, cx, &sort, (x0, y0));
    }
    // Bottom edge in the border colour at 60%.
    let border = color(Token::Border);
    fill(
        window,
        frame(x0, y0 + height - 1.0, f32::from(header.size.width), 1.0),
        border.alpha(border.a * 0.6),
    );
}
