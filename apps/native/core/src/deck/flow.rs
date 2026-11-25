//! A flowing document (Word, OpenDocument text, rich text) and how it is cut into pages. A
//! reader makes a `Flow`: the page's size and margins, and the document's blocks in order. Its
//! pages are laid out here into a deck the page view draws, each block placed under the one
//! before and moved to the next page when it does not fit.
//!
//! A paragraph or a table row is never split across pages: one taller than a page starts a page
//! and runs past its foot. Text wraps around nothing; a picture is a block of its own.

use super::layout::body_height;
use super::{
    Align, Anchor, Body, Colour, Deck, Fit, Form, Item, Paragraph, Placed, Shape, Slide, Source,
    Spacing, Stroke,
};
use crate::dialog::layout::Measure;
use crate::geometry::Rect;

/// A US Letter page with one-inch margins, for a document that gives no page.
pub const LETTER: Page = Page {
    width: 612.0,
    height: 792.0,
    margins: [72.0, 72.0, 72.0, 72.0],
};
/// The space kept clear inside a table cell: sides, then top and bottom.
const CELL_SIDE_INSET: f64 = 5.4;
const CELL_END_INSET: f64 = 2.0;
/// A table's lines.
const TABLE_LINE: Stroke = Stroke {
    colour: Colour::opaque(0x9e, 0x9e, 0x9e),
    width: 0.5,
};
/// The narrowest page a document is drawn with, so a damaged size can't make it vanish.
const PAGE_MIN_POINTS: f64 = 72.0;
/// A picture's size when the reader can't tell it, in points.
pub const PICTURE_DEFAULT_SIZE: (f64, f64) = (320.0, 240.0);

/// A page's size and its margins (left, top, right, bottom), in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    pub width: f64,
    pub height: f64,
    pub margins: [f64; 4],
}

impl Page {
    /// The page, no smaller than `PAGE_MIN_POINTS` and with margins that leave room for text.
    fn sane(self) -> Self {
        let width = self.width.max(PAGE_MIN_POINTS);
        let height = self.height.max(PAGE_MIN_POINTS);
        let [left, top, right, bottom] = self.margins.map(|margin| margin.max(0.0));
        let fits = left + right < width * 0.9 && top + bottom < height * 0.9;
        Self {
            width,
            height,
            margins: if fits {
                [left, top, right, bottom]
            } else {
                LETTER.margins
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub page: Page,
    pub blocks: Vec<Block>,
    pub pictures: Vec<Source>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Paragraph),
    Table(Table),
    /// One of the flow's pictures, at a size in points.
    Picture {
        picture: usize,
        width: f64,
        height: f64,
    },
    PageBreak,
}

/// A table: its columns' widths in points, and its rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub columns: Vec<f64>,
    pub rows: Vec<Vec<Cell>>,
}

/// A table cell: how many columns it spans, its fill, and its text. A cell merged into the one
/// above has no text and is drawn as part of its column's line.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub span: usize,
    pub fill: Option<Colour>,
    pub paragraphs: Vec<Paragraph>,
}

/// A paragraph with nothing in it at `size` points, for readers that need a placeholder.
pub fn empty_paragraph(size: f64) -> Paragraph {
    Paragraph {
        align: Align::Start,
        left: 0.0,
        indent: 0.0,
        line: Spacing::Share(1.0),
        before: Spacing::Points(0.0),
        after: Spacing::Points(0.0),
        bullet: None,
        size,
        runs: Vec::new(),
    }
}

fn body_of(paragraphs: Vec<Paragraph>, insets: [f64; 4]) -> Body {
    Body {
        insets,
        anchor: Anchor::Top,
        wraps: true,
        scale: 1.0,
        spacing_scale: 1.0,
        paragraphs,
    }
}

/// The pages being filled: those done, the one being filled, and how far down it is.
struct Pages {
    page: Page,
    done: Vec<Slide>,
    items: Vec<Item>,
    y: f64,
}

impl Pages {
    fn new(page: Page) -> Self {
        Self {
            page,
            done: Vec::new(),
            items: Vec::new(),
            y: page.margins[1],
        }
    }

    fn left(&self) -> f64 {
        self.page.margins[0]
    }

    fn width(&self) -> f64 {
        self.page.width - self.page.margins[0] - self.page.margins[2]
    }

    fn foot(&self) -> f64 {
        self.page.height - self.page.margins[3]
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn turn(&mut self) {
        let items = std::mem::take(&mut self.items);
        self.done.push(Slide {
            ground: Colour::WHITE,
            items,
        });
        self.y = self.page.margins[1];
    }

    /// Where a block `height` tall goes: under the last, or at the top of the next page when it
    /// does not fit on a page that already holds something.
    fn room_for(&mut self, height: f64) -> f64 {
        if self.y + height > self.foot() && !self.is_empty() {
            self.turn();
        }
        let top = self.y;
        self.y += height;
        top
    }

    fn finish(mut self) -> Vec<Slide> {
        if !self.items.is_empty() || self.done.is_empty() {
            self.turn();
        }
        self.done
    }
}

fn add_paragraph(pages: &mut Pages, paragraph: &Paragraph, measure: &mut Measure) {
    let body = body_of(vec![paragraph.clone()], [0.0; 4]);
    let height = body_height(&body, pages.width(), measure);
    let top = pages.room_for(height);
    pages.items.push(Item::Shape(Shape {
        frame: Rect::new(pages.left(), top, pages.width(), height),
        form: Form::Box,
        fill: None,
        stroke: None,
        text: Some(body),
    }));
}

fn add_picture(pages: &mut Pages, picture: usize, size: (f64, f64)) {
    let room = (pages.width(), pages.foot() - pages.page.margins[1]);
    let shrink = (room.0 / size.0).min(room.1 / size.1).clamp(0.0, 1.0);
    let (width, height) = (size.0 * shrink, size.1 * shrink);
    let top = pages.room_for(height);
    pages.items.push(Item::Picture(Placed {
        frame: Rect::new(pages.left(), top, width, height),
        picture,
        crop: [0.0; 4],
    }));
}

/// The table's column widths, scaled to the page's width when they are wider or not given.
fn column_widths(table: &Table, width: f64) -> Vec<f64> {
    let count = table
        .rows
        .iter()
        .map(|row| row.iter().map(|cell| cell.span).sum::<usize>())
        .max()
        .unwrap_or(0)
        .max(table.columns.len());
    let mut columns = table.columns.clone();
    let given: f64 = columns.iter().sum();
    let missing = count - columns.len();
    let even = if given < width && missing > 0 {
        (width - given) / missing as f64
    } else {
        width / count.max(1) as f64
    };
    columns.extend(std::iter::repeat_n(even, missing));
    let total: f64 = columns.iter().sum();
    if total > width || total <= 0.0 {
        let scale = width / total.max(1.0);
        return columns.iter().map(|column| column * scale).collect();
    }
    columns
}

const CELL_INSETS: [f64; 4] = [
    CELL_SIDE_INSET,
    CELL_END_INSET,
    CELL_SIDE_INSET,
    CELL_END_INSET,
];

fn add_row(pages: &mut Pages, row: &[Cell], columns: &[f64], measure: &mut Measure) {
    let mut spans = Vec::with_capacity(row.len());
    let mut first = 0;
    for cell in row {
        let last = (first + cell.span).min(columns.len());
        let width: f64 = columns[first.min(last)..last].iter().sum();
        spans.push((columns[..first.min(last)].iter().sum::<f64>(), width));
        first = last;
    }
    let bodies: Vec<Body> = row
        .iter()
        .map(|cell| body_of(cell.paragraphs.clone(), CELL_INSETS))
        .collect();
    let height = bodies
        .iter()
        .zip(&spans)
        .map(|(body, (_, width))| body_height(body, *width, measure))
        .fold(0.0, f64::max);
    let top = pages.room_for(height);
    for ((cell, body), (x, width)) in row.iter().zip(bodies).zip(spans) {
        pages.items.push(Item::Shape(Shape {
            frame: Rect::new(pages.left() + x, top, width, height),
            form: Form::Box,
            fill: cell.fill,
            stroke: Some(TABLE_LINE),
            text: (!body.paragraphs.is_empty()).then_some(body),
        }));
    }
}

fn add_table(pages: &mut Pages, table: &Table, measure: &mut Measure) {
    let columns = column_widths(table, pages.width());
    for row in &table.rows {
        add_row(pages, row, &columns, measure);
    }
}

/// Lays `flow` out into pages, setting its text with `measure`.
pub fn paginate(flow: Flow, measure: &mut Measure) -> Deck {
    let page = flow.page.sane();
    let mut pages = Pages::new(page);
    for block in &flow.blocks {
        match block {
            Block::Paragraph(paragraph) => add_paragraph(&mut pages, paragraph, measure),
            Block::Table(table) => add_table(&mut pages, table, measure),
            Block::Picture {
                picture,
                width,
                height,
            } => add_picture(&mut pages, *picture, (*width, *height)),
            Block::PageBreak if !pages.is_empty() => pages.turn(),
            Block::PageBreak => {}
        }
    }
    Deck {
        width: page.width,
        height: page.height,
        fit: Fit::Width,
        slides: pages.finish(),
        pictures: flow.pictures,
    }
}

/// A PNG's, JPEG's or GIF's size in pixels, read from its header; `None` for another format or
/// a damaged header.
pub fn picture_pixels(bytes: &[u8]) -> Option<(u32, u32)> {
    let be32 = |at: usize| Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let be16 = |at: usize| {
        Some(u32::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some((be32(16)?, be32(20)?));
    }
    if bytes.starts_with(b"GIF8") {
        return Some((le16(6)?, le16(8)?));
    }
    if !bytes.starts_with(&[0xff, 0xd8]) {
        return None;
    }
    // Walk the JPEG's segments to the first start-of-frame, which holds the size.
    let mut at = 2;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xff {
            return None;
        }
        let marker = bytes[at + 1];
        let frame = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
        if frame {
            return Some((be16(at + 7)?, be16(at + 5)?));
        }
        at += 2 + be16(at + 2)? as usize;
    }
    None
}

/// A picture's size in points from its pixels, read as 96 to the inch as browsers and Word do,
/// or `PICTURE_DEFAULT_SIZE` when its header can't be read.
pub fn picture_points(bytes: &[u8]) -> (f64, f64) {
    picture_pixels(bytes)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map_or(PICTURE_DEFAULT_SIZE, |(width, height)| {
            (f64::from(width) * 0.75, f64::from(height) * 0.75)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::card::Font;
    use crate::deck::Run;

    /// Every character ten points wide at size 20, and in proportion at other sizes.
    fn measure(text: &str, font: Font) -> f64 {
        text.chars().count() as f64 * font.size / 2.0
    }

    fn said(text: &str) -> Paragraph {
        Paragraph {
            runs: vec![Run {
                text: text.to_owned(),
                size: 20.0,
                bold: false,
                italic: false,
                underline: false,
                colour: Colour::BLACK,
            }],
            ..empty_paragraph(20.0)
        }
    }

    /// A page 200 by 100 with 10-point margins: 80 high for text, three 24-point lines.
    const SMALL: Page = Page {
        width: 200.0,
        height: 100.0,
        margins: [10.0; 4],
    };

    fn tops(slide: &Slide) -> Vec<f64> {
        slide
            .items
            .iter()
            .map(|item| match item {
                Item::Shape(shape) => shape.frame.y,
                Item::Picture(placed) => placed.frame.y,
                Item::Figure(_) => unreachable!("a flow draws no figures"),
            })
            .collect()
    }

    #[test]
    fn blocks_stack_and_move_to_the_next_page_when_they_do_not_fit() {
        let flow = Flow {
            page: SMALL,
            blocks: ["a", "b", "c", "d", "e"]
                .map(|text| Block::Paragraph(said(text)))
                .into(),
            pictures: Vec::new(),
        };
        let deck = paginate(flow, &mut measure);
        assert_eq!(
            (deck.width, deck.height, deck.fit),
            (200.0, 100.0, Fit::Width)
        );
        assert_eq!(deck.slides.len(), 2);
        assert_eq!(tops(&deck.slides[0]), [10.0, 34.0, 58.0]);
        assert_eq!(tops(&deck.slides[1]), [10.0, 34.0]);
    }

    #[test]
    fn a_page_break_starts_a_page_unless_the_page_is_empty() {
        let flow = Flow {
            page: SMALL,
            blocks: vec![
                Block::PageBreak,
                Block::Paragraph(said("a")),
                Block::PageBreak,
                Block::Paragraph(said("b")),
            ],
            pictures: Vec::new(),
        };
        assert_eq!(paginate(flow, &mut measure).slides.len(), 2);
        let empty = Flow {
            page: SMALL,
            blocks: Vec::new(),
            pictures: Vec::new(),
        };
        assert_eq!(paginate(empty, &mut measure).slides.len(), 1);
    }

    #[test]
    fn a_picture_shrinks_to_the_page_and_a_table_fills_its_width() {
        let cell = |text: &str, span| Cell {
            span,
            fill: None,
            paragraphs: vec![said(text)],
        };
        let flow = Flow {
            page: SMALL,
            blocks: vec![
                Block::Picture {
                    picture: 0,
                    width: 360.0,
                    height: 90.0,
                },
                Block::Table(Table {
                    columns: vec![30.0],
                    rows: vec![vec![cell("x", 1), cell("y", 2)]],
                }),
            ],
            pictures: vec![Source::Entry("a.png".to_owned())],
        };
        let deck = paginate(flow, &mut measure);
        let Item::Picture(placed) = &deck.slides[0].items[0] else {
            panic!("the picture comes first");
        };
        assert_eq!(placed.frame, Rect::new(10.0, 10.0, 180.0, 45.0));
        let frames: Vec<Rect> = deck.slides[0].items[1..]
            .iter()
            .map(|item| match item {
                Item::Shape(shape) => shape.frame,
                Item::Picture(_) | Item::Figure(_) => unreachable!(),
            })
            .collect();
        // 30 given, the two missing columns share the other 150; a row is its text's height
        // and its insets.
        assert_eq!(
            frames,
            [
                Rect::new(10.0, 55.0, 30.0, 28.0),
                Rect::new(40.0, 55.0, 150.0, 28.0)
            ]
        );
    }

    #[test]
    fn a_damaged_page_is_made_usable() {
        let page = Page {
            width: 0.0,
            height: 500.0,
            margins: [400.0, 0.0, 400.0, 0.0],
        }
        .sane();
        assert_eq!(
            (page.width, page.margins),
            (PAGE_MIN_POINTS, LETTER.margins)
        );
    }

    #[test]
    fn a_picture_header_gives_its_size() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 0, 40, 0, 0, 0, 20]);
        assert_eq!(picture_pixels(&png), Some((40, 20)));
        assert_eq!(picture_points(&png), (30.0, 15.0));
        let gif = b"GIF89a\x10\x00\x08\x00";
        assert_eq!(picture_pixels(gif), Some((16, 8)));
        // An application segment, then a baseline frame 3 high and 5 wide.
        let jpeg = [
            0xff, 0xd8, 0xff, 0xe0, 0, 4, 0, 0, 0xff, 0xc0, 0, 11, 8, 0, 3, 0, 5, 1, 1, 0x11, 0,
        ];
        assert_eq!(picture_pixels(&jpeg), Some((5, 3)));
        assert_eq!(picture_points(b"nothing"), PICTURE_DEFAULT_SIZE);
    }
}
