//! Layout of the open page in the editor's type: where each block sits, the marks it draws
//! (`board::card::Mark`, rendered by the same code as cards), where its text fields go, and
//! hit testing. A read-only page (a plain-text file shown as a page) has its own layout
//! (`lay_out_document`).
//!
//! The typesetter (`typeset`) decides wrapping: the page asks a `Measure` how many lines some
//! text takes, as a page card does, and stacks the blocks accordingly.

use crate::board::card::{list_marker, plain, styled, Font, Ink, Lines, Mark, Measure, Run, Words};
use crate::board::Frame;
use crate::glyph::Glyph;
use crate::page::inline::Segment;
use crate::page::media::{picture_drawn_size, ImageShows};
use crate::page::{Block, BlockKind};
use crate::theme::{Paint, Token};

/// Pictures are drawn from a thumbnail at most this many pixels on a side.
const PICTURE_PIXELS: u32 = 2048;
/// An image takes its own line with this much space above and below.
const IMAGE_MARGIN: f64 = 6.0;
/// Corner radius of backgrounds, pictures and selected rows.
const RADIUS: f64 = 6.0;

/// The input to layout: the open page as last read.
pub struct Sheet<'a> {
    pub title: &'a str,
    pub blocks: &'a [Block],
    pub segments: &'a [Vec<Segment>],
    pub numbers: &'a [Option<u32>],
    /// A plain-text file shown as a page, which cannot be edited.
    pub read_only: bool,
    /// The interface language code for the page's own UI text.
    pub language: &'a str,
}

/// The space the page is laid out in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Room {
    pub width: f64,
    /// The height of the visible viewport.
    pub viewport: f64,
    /// The width of the window the page is in.
    pub window_width: f64,
}

/// A player block's box as the shell placed it: where its space ends, its box, and whether it is
/// drawn as a player (one with nothing to play shows its text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerPlaced {
    pub end: f64,
    pub frame: Frame,
    pub drawn: bool,
}

/// Places player block `block` with its box's top at `top`, in a column starting at `side` and
/// `column` wide.
pub type PlacePlayer<'a> = dyn FnMut(usize, f64, f64, f64) -> PlayerPlaced + 'a;

/// What image segment `segment` of block `block` shows.
pub type ShowImage<'a> = dyn FnMut(usize, usize) -> ImageShows + 'a;

/// A text field's placement.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    /// The block's index, or -1 for the title.
    pub block: i64,
    pub frame: Frame,
    pub font: Font,
    pub line_height: f64,
    /// Letter spacing, used by the title.
    pub tracking: f64,
    /// A table cell's row and column.
    pub cell: Option<(usize, usize)>,
    /// A board media block's caption: one line of plain text.
    pub caption: bool,
    /// The background behind the text when it differs from the page's, as in a code block.
    pub ground: Option<Token>,
}

/// One block's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub frame: Frame,
    /// `kind_code`.
    pub kind: u32,
    pub level: u32,
    /// The page's first block.
    pub first: bool,
}

/// When a mark is drawn: always, or depending on whether its slot is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Always,
    /// While slot `0` is not being edited: its text, which the field draws while it is.
    Resting(usize),
    /// While slot `0` is being edited.
    Typing(usize),
    /// While slot `0` is being edited and empty: a placeholder.
    TypingEmpty(usize),
}

/// The page laid out.
pub struct Layout {
    pub marks: Vec<(Mark, Shown, Frame)>,
    pub slots: Vec<Slot>,
    pub blocks: Vec<Placed>,
    /// Each to-do's marker, where a press ticks it.
    pub todos: Vec<(usize, Frame)>,
    /// Each board media block's frame.
    pub media: Vec<(usize, Frame)>,
    /// Each player block's box.
    pub players: Vec<(usize, PlayerPlaced)>,
    /// The side margin.
    pub side: f64,
    /// The page's whole height.
    pub height: f64,
}

/// A block kind's FFI number (`NrPagePlaced::kind`).
pub fn kind_code(kind: &BlockKind) -> u32 {
    match kind {
        BlockKind::Paragraph { .. } => 0,
        BlockKind::Heading { .. } => 1,
        BlockKind::Bullet { .. } => 2,
        BlockKind::Numbered { .. } => 3,
        BlockKind::Todo { .. } => 4,
        BlockKind::Quote { .. } => 5,
        BlockKind::Code { .. } => 6,
        BlockKind::Html { .. } => 7,
        BlockKind::Divider => 8,
        BlockKind::Table { .. } => 9,
        BlockKind::Media { .. } => 10,
        BlockKind::Player { .. } => 11,
    }
}

fn frame(x: f64, y: f64, width: f64, height: f64) -> Frame {
    Frame {
        x,
        y,
        width,
        height,
    }
}

fn inset(frame: Frame, by: f64) -> Frame {
    Frame {
        x: frame.x + by,
        y: frame.y + by,
        width: frame.width - by * 2.0,
        height: frame.height - by * 2.0,
    }
}

fn contains(frame: Frame, x: f64, y: f64) -> bool {
    frame.x <= x && x <= frame.x + frame.width && frame.y <= y && y <= frame.y + frame.height
}

fn fill(frame: Frame, radius: f64, token: Token, opacity: f64) -> Mark {
    Mark::Fill {
        frame,
        radius,
        ink: Ink {
            paint: token.into(),
            opacity,
        },
    }
}

/// Text in one font, colour and line height, wrapped to `width`.
fn set(x: f64, top: f64, width: f64, line_height: f64, line_font: Font, runs: Vec<Run>) -> Words {
    Words {
        x,
        top,
        width,
        wraps: true,
        line_height,
        line_font,
        tab_spaces: 0,
        clip: None,
        runs,
    }
}

/// The type settings for a block's text.
#[derive(Clone, Copy)]
struct Type {
    size: f64,
    weight: f64,
    line_height: f64,
    color: Token,
    struck: bool,
}

/// Lays out and records marks, stacking text by the typesetter's measurements.
struct Setter<'a, 'b, 'c> {
    measure: &'a mut Measure<'b>,
    show_image: &'a mut ShowImage<'c>,
    marks: Vec<(Mark, Shown, Frame)>,
}

impl Setter<'_, '_, '_> {
    fn lines(&mut self, words: &Words) -> usize {
        (self.measure)(words).lines
    }

    fn mark(&mut self, mark: Mark, shown: Shown, bounds: Frame) {
        self.marks.push((mark, shown, bounds));
    }

    /// Draws `words` and returns their total line height.
    fn words(&mut self, words: Words, shown: Shown) -> f64 {
        let height = self.lines(&words) as f64 * words.line_height;
        let bounds = words
            .clip
            .unwrap_or(frame(words.x, words.top, words.width, height));
        self.mark(Mark::PageWords(words), shown, bounds);
        height
    }

    /// Draws one block's text from (`x`, `top`), `width` wide, and returns its height (the sum of
    /// its line heights, rounded up). Each image takes its own line: text before it ends its line
    /// and text after it starts a new one. After a trailing image the block continues with an
    /// empty line, where the caret sits.
    fn rich(
        &mut self,
        block: usize,
        segments: &[Segment],
        x: f64,
        top: f64,
        width: f64,
        kind: Type,
    ) -> f64 {
        let line_font = Font::sans(kind.size, Font::REGULAR);
        let mut runs: Vec<Run> = Vec::new();
        let mut used = 0.0;
        let flush = |setter: &mut Self, runs: &mut Vec<Run>, used: &mut f64| {
            // Inline code's padding is space after the preceding letter, so there is none at the
            // start of a line.
            if matches!(runs.first(), Some(Run::Space { .. })) {
                runs.remove(0);
            }
            if runs.is_empty() {
                return;
            }
            *used += setter.words(
                set(
                    x,
                    top + *used,
                    width,
                    kind.line_height,
                    line_font,
                    std::mem::take(runs),
                ),
                Shown::Always,
            );
        };
        let mut after_image = false;
        for (index, segment) in segments.iter().enumerate() {
            if segment.image.is_none() {
                after_image = false;
                runs.extend(styled(
                    std::slice::from_ref(segment),
                    kind.size,
                    kind.weight,
                    kind.color,
                    kind.struck,
                    Lines::Kept,
                ));
                continue;
            }
            flush(self, &mut runs, &mut used);
            after_image = true;
            let line_top = top + used;
            used += match (self.show_image)(block, index) {
                ImageShows::Picture {
                    id,
                    size: Some(size),
                } => {
                    let (drawn_width, drawn_height) = picture_drawn_size(size, width);
                    let picture = frame(x, line_top + IMAGE_MARGIN, drawn_width, drawn_height);
                    self.mark(
                        Mark::Image {
                            frame: picture,
                            id,
                            radius: RADIUS,
                            ground: Token::Muted,
                            pixels: PICTURE_PIXELS,
                        },
                        Shown::Always,
                        picture,
                    );
                    drawn_height + IMAGE_MARGIN * 2.0
                }
                // A picture the platform cannot read is a broken `<img>`: an empty line.
                ImageShows::Picture { size: None, .. } => kind.line_height + IMAGE_MARGIN * 2.0,
                // A video or sound shows its alt text in the line's own type.
                ImageShows::Words(words) => {
                    let font = Font::sans(kind.size, kind.weight);
                    self.words(
                        set(
                            x,
                            line_top,
                            width,
                            kind.line_height + IMAGE_MARGIN * 2.0,
                            line_font,
                            alt(&words, font, kind.color),
                        ),
                        Shown::Always,
                    )
                }
                // No such file: its alt text or address, small and muted.
                ImageShows::Missing(words) => {
                    let font = Font::sans(13.0, Font::REGULAR);
                    self.words(
                        set(
                            x,
                            line_top,
                            width,
                            20.8 + IMAGE_MARGIN * 2.0,
                            font,
                            alt(&words, font, Token::MutedForeground),
                        ),
                        Shown::Always,
                    )
                }
            };
        }
        flush(self, &mut runs, &mut used);
        if after_image {
            used += kind.line_height;
        }
        used.ceil()
    }
}

/// An image's alt text, or a space that holds its line open when there is none.
fn alt(words: &str, font: Font, color: Token) -> Vec<Run> {
    plain(if words.is_empty() { "\u{a0}" } else { words }, font, color)
}

/// Lays `sheet` out in `room`.
pub fn lay_out(
    sheet: &Sheet,
    room: Room,
    measure: &mut Measure,
    show_image: &mut ShowImage,
    place_player: &mut PlacePlayer,
) -> Layout {
    let mut setter = Setter {
        measure,
        show_image,
        marks: Vec::new(),
    };
    if sheet.read_only {
        return lay_out_document(sheet, room, &mut setter);
    }
    // The page margins: 6% of the width, clamped to 56–96 pt.
    let side = (room.width * 0.06).clamp(56.0, 96.0);
    let mut writer = PageWriter {
        setter,
        place_player,
        layout: Layout {
            marks: Vec::new(),
            slots: Vec::new(),
            blocks: Vec::new(),
            todos: Vec::new(),
            media: Vec::new(),
            players: Vec::new(),
            side,
            height: 0.0,
        },
        side,
        column: room.width - side * 2.0,
        width: room.width,
        y: 40.0,
    };
    writer.put_title(sheet.title);
    for (index, block) in sheet.blocks.iter().enumerate() {
        writer.put_block(sheet, index, block);
    }
    let mut layout = writer.layout;
    // Overscroll so the last line can scroll up: 30% of the viewport height.
    layout.height = room.viewport.max(writer.y + room.viewport * 0.3);
    layout.marks = writer.setter.marks;
    layout
}

impl Type {
    /// Text in the page's foreground colour, not struck through.
    fn foreground(size: f64, weight: f64, line_height: f64) -> Self {
        Self {
            size,
            weight,
            line_height,
            color: Token::Foreground,
            struck: false,
        }
    }
}

/// A table's shape: its rows and columns, and how wide each cell is.
#[derive(Clone, Copy)]
struct Grid {
    rows: usize,
    columns: usize,
    cell_width: f64,
}

impl Grid {
    /// The edges cell (`row`, `place`) at `box_` draws. Borders are shared as in a collapsed
    /// table: each cell draws its top and left, and the last row and column also their far edges.
    fn cell_edges(self, box_: Frame, (row, place): (usize, usize)) -> Vec<Frame> {
        let mut edges = vec![
            frame(box_.x, box_.y, box_.width, 1.0),
            frame(box_.x, box_.y, 1.0, box_.height),
        ];
        if row == self.rows - 1 {
            edges.push(frame(box_.x, box_.y + box_.height, box_.width + 1.0, 1.0));
        }
        if place == self.columns - 1 {
            edges.push(frame(box_.x + box_.width, box_.y, 1.0, box_.height));
        }
        edges
    }
}

/// Table cell text: 8 pt from either side, 6 pt down, on 21 pt lines, wrapped in the cell. A row
/// is its lines plus 13 pt.
const CELL_IN: f64 = 8.0;
const CELL_DOWN: f64 = 6.0;
const CELL_LINE: f64 = 21.0;
const CELL_ROOM: f64 = 13.0;

/// The header row's words are semibold.
fn cell_font(row: usize) -> Font {
    Font::sans(
        14.0,
        if row == 0 {
            Font::SEMIBOLD
        } else {
            Font::REGULAR
        },
    )
}

/// Cell `text` of row `row` set in the cell at `box_`.
fn cell_words(grid: Grid, box_: Frame, row: usize, text: &str) -> Words {
    Words {
        clip: Some(box_),
        ..set(
            box_.x + CELL_IN,
            box_.y + CELL_DOWN,
            grid.cell_width - 2.0 * CELL_IN,
            CELL_LINE,
            Font::sans(14.0, Font::REGULAR),
            plain(text, cell_font(row), Token::Foreground),
        )
    }
}

/// Stacks an editable page's blocks down its column; `y` is where the next one starts.
struct PageWriter<'a, 'b, 'c, 'd> {
    setter: Setter<'a, 'b, 'c>,
    place_player: &'a mut PlacePlayer<'d>,
    layout: Layout,
    /// The side margin, the column width between the margins, and the full width.
    side: f64,
    column: f64,
    width: f64,
    y: f64,
}

impl PageWriter<'_, '_, '_, '_> {
    /// The title is edited like a block, with index -1.
    fn put_title(&mut self, title: &str) {
        let (side, column) = (self.side, self.column);
        let title_font = Font::sans(32.0, Font::BOLD);
        let mut runs = plain(title, title_font, Token::Foreground);
        if let Some(Run::Words { style, .. }) = runs.first_mut() {
            style.tracking = -0.32;
        }
        let title_height = if title.is_empty() {
            0.0
        } else {
            self.setter
                .words(
                    set(
                        side,
                        self.y,
                        column,
                        38.4,
                        Font::sans(32.0, Font::REGULAR),
                        runs,
                    ),
                    Shown::Always,
                )
                .ceil()
        };
        self.layout.slots.push(Slot {
            block: -1,
            frame: frame(side, self.y, column, title_height),
            font: title_font,
            line_height: 38.4,
            tracking: -0.32,
            cell: None,
            caption: false,
            ground: None,
        });
        self.y += title_height + 12.0;
    }

    /// Lays out block `index` and its frame.
    fn put_block(&mut self, sheet: &Sheet, index: usize, block: &Block) {
        let segments = &sheet.segments[index];
        // Every block has 2 pt above and below; its text sits 1 pt down and 2 pt in.
        let block_top = self.y;
        self.y += 2.0;
        match &block.kind {
            BlockKind::Heading { level, .. } => self.put_heading(index, segments, *level),
            BlockKind::Bullet { indent, .. }
            | BlockKind::Numbered { indent, .. }
            | BlockKind::Todo { indent, .. } => {
                self.put_list_item(index, segments, &block.kind, *indent, sheet.numbers[index]);
            }
            BlockKind::Quote { .. } => self.put_quote(index, segments),
            BlockKind::Code { text: source, .. } | BlockKind::Html { text: source } => {
                self.put_code(index, source);
            }
            BlockKind::Divider => self.put_divider(block_top),
            BlockKind::Table { rows, .. } => self.put_table(index, rows),
            BlockKind::Media { caption, .. } => {
                self.put_media(index, block_top, caption, sheet.language);
            }
            BlockKind::Player { .. } => self.put_player(index, block_top),
            BlockKind::Paragraph { .. } => {
                let (x, width) = (self.side + 2.0, self.column - 4.0);
                let kind = Type::foreground(15.0, 400.0, 24.0);
                let height = self.put_body_words(index, segments, x, width, kind);
                self.y += height + 2.0;
            }
        }
        self.y += 2.0;
        let level = if let BlockKind::Heading { level, .. } = block.kind {
            u32::from(level)
        } else {
            0
        };
        self.layout.blocks.push(Placed {
            frame: frame(
                self.side - 6.0,
                block_top,
                self.column + 12.0,
                self.y - block_top,
            ),
            kind: kind_code(&block.kind),
            level,
            first: index == 0,
        });
    }

    /// A block's editable text from `x`, 1 pt below `y`, `width` wide. Returns its height, at least
    /// one line.
    fn put_body_words(
        &mut self,
        index: usize,
        segments: &[Segment],
        x: f64,
        width: f64,
        kind: Type,
    ) -> f64 {
        let height = self
            .setter
            .rich(index, segments, x, self.y + 1.0, width, kind)
            .max(24.0);
        self.layout.slots.push(Slot {
            block: index as i64,
            frame: frame(x, self.y + 1.0, width, height),
            ..words_slot(15.0, 24.0)
        });
        height
    }

    fn put_heading(&mut self, index: usize, segments: &[Segment], level: u8) {
        let size = match level {
            1 => 28.0,
            2 => 22.0,
            _ => 18.0,
        };
        // 14 pt above instead of a block's 2 pt, and none when it is the page's first.
        self.y += if index == 0 { -2.0 } else { 12.0 };
        let line = (size * 1.3_f64).round();
        let (x, top, width) = (self.side + 2.0, self.y + 1.0, self.column - 4.0);
        let measured = self
            .setter
            .rich(
                index,
                segments,
                x,
                top,
                width,
                Type::foreground(size, 600.0, line),
            )
            .max(line);
        self.layout.slots.push(Slot {
            block: index as i64,
            frame: frame(x, top, width, measured),
            font: Font::sans(size, Font::SEMIBOLD),
            line_height: line,
            tracking: 0.0,
            cell: None,
            caption: false,
            ground: None,
        });
        // Each line is 1.3× its size, unrounded, with 1 pt above and below.
        self.y += (measured / line).floor().max(1.0) * size * 1.3 + 2.0;
    }

    /// A bullet, numbered item or to-do: its marker in a 24 pt slot `indent` levels in, its text
    /// after it.
    fn put_list_item(
        &mut self,
        index: usize,
        segments: &[Segment],
        kind: &BlockKind,
        indent: u8,
        number: Option<u32>,
    ) {
        let indent = f64::from(indent) * 24.0;
        let done = matches!(kind, BlockKind::Todo { checked: true, .. });
        let text_x = self.side + indent + 24.0 + 4.0;
        let text_width = self.width - self.side - text_x - 4.0;
        let words = Type {
            color: if done {
                Token::MutedForeground
            } else {
                Token::Foreground
            },
            struck: done,
            ..Type::foreground(15.0, 400.0, 24.0)
        };
        let height = self.put_body_words(index, segments, text_x + 2.0, text_width, words);
        let marker = frame(self.side + indent, self.y, 24.0, 26.0);
        if let BlockKind::Todo { checked, .. } = kind {
            self.layout.todos.push((index, marker));
            let tick = frame(marker.x + 4.0, marker.y + 5.0, 16.0, 16.0);
            if *checked {
                self.setter.mark_ticked_box(tick);
            } else {
                self.setter.mark_empty_box(tick);
            }
        } else {
            self.setter.mark_list_label(kind, number, marker);
        }
        self.y += height + 2.0;
    }

    /// The 3 pt rule sits at the text's left edge, and the text starts 14 pt after it.
    fn put_quote(&mut self, index: usize, segments: &[Segment]) {
        let width = self.column - 4.0 - 17.0;
        let kind = Type::foreground(15.0, 400.0, 24.0);
        let height = self.put_body_words(index, segments, self.side + 17.0, width, kind);
        let rule = frame(self.side, self.y, 3.0, height + 2.0);
        self.setter
            .mark(fill(rule, 0.0, Token::Foreground, 1.0), Shown::Always, rule);
        self.y += height + 2.0;
    }

    /// The source in monospace, 12 pt inside its background above and below and 16 pt in, on as
    /// many 20.15 pt lines as it needs.
    fn put_code(&mut self, index: usize, source: &str) {
        let (side, column, y) = (self.side, self.column, self.y);
        let font = Font::mono(13.0);
        let words = set(
            side + 16.0,
            y + 12.0,
            column - 32.0,
            20.15,
            font,
            plain(source, font, Token::Foreground),
        );
        let lines = if source.is_empty() {
            0
        } else {
            self.setter.lines(&words)
        };
        let height = ((lines as f64 * 20.15).ceil() / 20.15).floor().max(1.0) * 20.15;
        let ground = frame(side, y, column, height + 24.0);
        self.setter.mark(
            fill(ground, RADIUS, Token::Muted, 1.0),
            Shown::Always,
            ground,
        );
        let bounds = frame(words.x, words.top, words.width, height);
        self.setter
            .mark(Mark::PageWords(words), Shown::Always, bounds);
        self.layout.slots.push(Slot {
            block: index as i64,
            frame: frame(side + 16.0, y + 12.0, column - 32.0, height),
            font,
            line_height: 20.15,
            tracking: 0.0,
            cell: None,
            caption: false,
            ground: Some(Token::Muted),
        });
        self.y += height + 24.0;
    }

    /// 21 pt tall, with its line 10 pt down.
    fn put_divider(&mut self, block_top: f64) {
        let line = frame(self.side, block_top + 10.0, self.column, 1.0);
        self.setter
            .mark(fill(line, 0.0, Token::Border, 1.0), Shown::Always, line);
        self.y = block_top + 21.0 - 2.0;
    }

    fn put_table(&mut self, index: usize, rows: &[Vec<String>]) {
        let columns = rows.first().map_or(0, Vec::len);
        if columns == 0 {
            return;
        }
        // 24 pt left at its right for the row's remove mark; it sits 4 pt down.
        let grid = Grid {
            rows: rows.len(),
            columns,
            cell_width: ((self.column - 24.0) / columns as f64).floor(),
        };
        self.y += 2.0;
        for (row, cells) in rows.iter().enumerate() {
            // A row is as tall as its longest wrapped cell, so typing in a cell pushes the rows
            // below down instead of overlapping them.
            let lines = cells
                .iter()
                .map(|cell| {
                    self.setter.lines(&cell_words(
                        grid,
                        frame(0.0, 0.0, grid.cell_width, 0.0),
                        row,
                        cell,
                    ))
                })
                .max()
                .unwrap_or(1)
                .max(1);
            let height = lines as f64 * CELL_LINE + CELL_ROOM;
            for (place, cell) in cells.iter().enumerate() {
                self.put_table_cell(index, grid, (row, place), height, cell);
            }
            self.y += height;
        }
        // The last row's bottom edge, the "+ Row" and "+ Column" strip (26 pt, shown while
        // the pointer is over the table), then 4 pt.
        self.y += 1.0 + 26.0 + 4.0 - 2.0;
    }

    /// Cell (`row`, `place`) of table block `index`, in the `height`-tall row at `y`. The first row
    /// is the header.
    fn put_table_cell(
        &mut self,
        index: usize,
        grid: Grid,
        (row, place): (usize, usize),
        height: f64,
        cell: &str,
    ) {
        let box_ = frame(
            self.side + place as f64 * grid.cell_width,
            self.y,
            grid.cell_width,
            height,
        );
        let slot = self.layout.slots.len();
        self.layout.slots.push(Slot {
            block: index as i64,
            frame: frame(
                box_.x + CELL_IN,
                box_.y + CELL_DOWN,
                grid.cell_width - 2.0 * CELL_IN,
                height - CELL_ROOM,
            ),
            font: cell_font(row),
            line_height: CELL_LINE,
            tracking: 0.0,
            cell: Some((row, place)),
            caption: false,
            ground: None,
        });
        if row == 0 {
            self.setter
                .mark(fill(box_, 0.0, Token::Secondary, 1.0), Shown::Always, box_);
        }
        // The cell being edited sits on 8% of the text colour; its field draws its text.
        self.setter.mark(
            fill(box_, 0.0, Token::Foreground, 0.08),
            Shown::Typing(slot),
            box_,
        );
        for edge in grid.cell_edges(box_, (row, place)) {
            self.setter
                .mark(fill(edge, 0.0, Token::Border, 1.0), Shown::Always, edge);
        }
        self.setter
            .words(cell_words(grid, box_, row, cell), Shown::Resting(slot));
    }

    /// Board media: 6 pt above and below, a frame on the muted background, then the caption field 4
    /// pt below it. The asset id resolves to nothing in a drive, so the frame reads "Board media".
    fn put_media(&mut self, index: usize, block_top: f64, caption: &str, language: &str) {
        let figure = frame(self.side, block_top + 6.0, self.column, 67.5);
        let field = frame(self.side, figure.y + figure.height + 4.0, self.column, 19.5);
        self.layout.media.push((index, figure));
        let slot = self.layout.slots.len();
        self.layout.slots.push(Slot {
            block: index as i64,
            frame: field,
            caption: true,
            ..words_slot(13.0, 19.5)
        });
        self.setter.mark(
            fill(figure, RADIUS, Token::Muted, 1.0),
            Shown::Always,
            figure,
        );
        self.setter.mark_media_label(figure, language);
        self.setter.mark_caption(slot, field, caption, language);
        // The field sits in a line of the figure's type, 24.75 pt including its margin.
        self.y = figure.y + figure.height + 24.75 + 6.0 - 2.0;
    }

    /// A `<video>` or `<audio>`, placed by the shell, with 6 pt above and below like media.
    fn put_player(&mut self, index: usize, block_top: f64) {
        let placed = (self.place_player)(index, block_top + 6.0, self.side, self.column);
        self.layout.players.push((index, placed));
        self.y = placed.end + 6.0 - 2.0;
    }
}

impl Setter<'_, '_, '_> {
    /// The width of `runs` on one unwrapped line.
    fn widest(&mut self, line_height: f64, font: Font, runs: Vec<Run>) -> f64 {
        let alone = Words {
            wraps: false,
            ..set(0.0, 0.0, f64::INFINITY, line_height, font, runs)
        };
        (self.measure)(&alone).widest
    }

    /// A done to-do's box at `tick`: filled, with a bold 12 pt tick, in the page colour.
    fn mark_ticked_box(&mut self, tick: Frame) {
        self.mark(
            fill(tick, RADIUS, Token::Foreground, 1.0),
            Shown::Always,
            tick,
        );
        self.mark(
            Mark::Glyph {
                frame: inset(tick, 2.0),
                icon: Glyph::Check,
                color: Paint::Theme(Token::Background),
            },
            Shown::Always,
            tick,
        );
    }

    /// An open to-do's box at `tick`: an outline.
    fn mark_empty_box(&mut self, tick: Frame) {
        self.mark(
            Mark::Stroke {
                frame: inset(tick, 0.75),
                radius: 5.25,
                ink: Ink::of(Token::MutedForeground),
                width: 1.5,
            },
            Shown::Always,
            tick,
        );
    }

    /// A bullet or number, centred in `marker`.
    fn mark_list_label(&mut self, kind: &BlockKind, number: Option<u32>, marker: Frame) {
        let label = list_marker(kind, number).unwrap_or_default();
        let font = Font::sans(15.0, Font::REGULAR);
        let width = self.widest(24.0, font, plain(&label, font, Token::Foreground));
        let max_width = if matches!(kind, BlockKind::Bullet { .. }) {
            24.0
        } else {
            40.0
        };
        self.mark(
            Mark::Line {
                text: label,
                font,
                color: Token::Foreground.into(),
                x: marker.x + marker.width / 2.0 - width / 2.0,
                top: marker.y + 1.0,
                line_height: 24.0,
                max_width,
            },
            Shown::Always,
            marker,
        );
    }

    /// "Board media" in the middle of a media block's `figure`.
    fn mark_media_label(&mut self, figure: Frame, language: &str) {
        let font = Font::sans(13.0, Font::REGULAR);
        let label = crate::strings::text(language, crate::strings::key::PAGE_MEDIA_BOARD);
        let width = self.widest(19.5, font, plain(label, font, Token::MutedForeground));
        self.mark(
            Mark::Line {
                text: label.to_owned(),
                font,
                color: Token::MutedForeground.into(),
                x: figure.x + figure.width / 2.0 - width / 2.0,
                top: figure.y + 24.0,
                line_height: 19.5,
                max_width: figure.width,
            },
            Shown::Always,
            figure,
        );
    }

    /// A media block's caption in `field`, slot `slot`. While the field is being edited it draws
    /// its own text, and the page draws only the placeholder underneath, and only while it is
    /// empty.
    fn mark_caption(&mut self, slot: usize, field: Frame, caption: &str, language: &str) {
        let font = Font::sans(13.0, Font::REGULAR);
        let line = |runs: Vec<Run>| Words {
            wraps: false,
            clip: Some(field),
            ..set(field.x, field.y, f64::INFINITY, 19.5, font, runs)
        };
        let placeholder = || {
            let mut runs = plain(
                crate::strings::text(language, crate::strings::key::PAGE_CAPTION_PLACEHOLDER),
                font,
                Token::MutedForeground,
            );
            if let Some(Run::Words { style, .. }) = runs.first_mut() {
                style.opacity = 0.5;
            }
            runs
        };
        let resting = if caption.is_empty() {
            placeholder()
        } else {
            plain(caption, font, Token::MutedForeground)
        };
        // One line that never wraps, like an input field.
        self.mark(Mark::Words(line(resting)), Shown::Resting(slot), field);
        self.mark(
            Mark::Words(line(placeholder())),
            Shown::TypingEmpty(slot),
            field,
        );
    }
}

/// The slot for a block's plain text in Inter at `size`.
fn words_slot(size: f64, line_height: f64) -> Slot {
    Slot {
        block: -1,
        frame: frame(0.0, 0.0, 0.0, 0.0),
        font: Font::sans(size, Font::REGULAR),
        line_height,
        tracking: 0.0,
        cell: None,
        caption: false,
        ground: None,
    }
}

/// A plain-text file shown as a page: 48 pt top margin, 5% of the window either side (24–64 pt),
/// the name at 32 pt with 28 pt below it, then each paragraph at 15 pt on 24.75 pt lines, 13.6 pt
/// apart. Nothing is editable, and a player shows as text since there is no folder to play from.
fn lay_out_document(sheet: &Sheet, room: Room, setter: &mut Setter) -> Layout {
    let side = (room.window_width * 0.05).clamp(24.0, 64.0);
    let column = room.width - side * 2.0;
    let mut y = 48.0;
    let title_height = if sheet.title.is_empty() {
        0.0
    } else {
        let font = Font::sans(32.0, 650.0);
        setter
            .words(
                set(
                    side,
                    y,
                    column,
                    38.4,
                    Font::sans(32.0, Font::REGULAR),
                    plain(sheet.title, font, Token::Foreground),
                ),
                Shown::Always,
            )
            .ceil()
    };
    y += title_height + 28.0;
    let kind = Type {
        size: 15.0,
        weight: 400.0,
        line_height: 24.75,
        color: Token::Foreground,
        struck: false,
    };
    for (index, block) in sheet.blocks.iter().enumerate() {
        let height = match &block.kind {
            BlockKind::Player { html } if !html.is_empty() => {
                let font = Font::sans(15.0, Font::REGULAR);
                setter
                    .words(
                        set(
                            side,
                            y,
                            column,
                            24.75,
                            font,
                            plain(html, font, Token::Foreground),
                        ),
                        Shown::Always,
                    )
                    .ceil()
            }
            BlockKind::Player { .. } => 0.0,
            _ => setter.rich(index, &sheet.segments[index], side, y, column, kind),
        };
        y += height.max(24.75) + 13.6;
    }
    Layout {
        marks: std::mem::take(&mut setter.marks),
        slots: Vec::new(),
        blocks: Vec::new(),
        todos: Vec::new(),
        media: Vec::new(),
        players: Vec::new(),
        side,
        height: room.viewport.max(y - 13.6 + 48.0),
    }
}

/// Hit test result (`Layout::hit`). Each field is -1 for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The block whose row, including its gutter, is under the point.
    pub row: i64,
    /// The gutter button under the point: 1 for `+`, 2 for the handle, 0 for none.
    pub button: u32,
    /// The block whose full frame contains the point.
    pub block: i64,
    /// The to-do whose marker, with 4 pt of slop, contains the point.
    pub todo: i64,
    /// The slot (`Layout::slots`) whose text, with 2 pt of slop, contains the point.
    pub slot: i64,
    /// The table cell slot under the point, with 8 pt and 6 pt of slop, for its menu.
    pub cell: i64,
    /// The board media block whose frame contains the point.
    pub media: i64,
    /// The player block whose frame contains the point.
    pub player: i64,
    /// The point is below the last block's text.
    pub below: bool,
}

/// Pointer and keyboard state that the page draws over and under its blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlay {
    /// The selected whole blocks, first to last.
    pub chosen: Option<(usize, usize)>,
    /// The block whose gutter shows, and its highlighted button (`Hit::button`).
    pub hovered: Option<usize>,
    pub button: u32,
    /// Where dragged blocks would land: before or after the block.
    pub drop: Option<(usize, bool)>,
    /// The media block reached by keyboard, which shows a ring.
    pub ringed: Option<usize>,
    /// The page's width.
    pub width: f64,
}

impl Layout {
    fn block_frame(&self, block: usize) -> Option<Frame> {
        self.blocks.get(block).map(|placed| placed.frame)
    }

    /// The two gutter buttons of a block, `+` then the handle: 20 by 24 pt, 1 pt apart, ending 4 pt
    /// left of the side margin, centred on the block's first line. The space above that line and
    /// its height come from the stylesheet, per kind.
    pub fn gutter(&self, block: usize) -> Option<(Frame, Frame)> {
        let placed = self.blocks.get(block)?;
        let (offset, line) = match placed.kind {
            1 => (
                if placed.first { 1.0 } else { 15.0 },
                [36.4, 28.6, 23.4][(placed.level.clamp(1, 3) - 1) as usize],
            ),
            6 | 7 => (14.0, 20.16),
            8 => (0.0, 21.0),
            9 => (4.0, 34.0),
            10 | 11 => (6.0, 24.0),
            _ => (3.0, 24.0),
        };
        let top = (placed.frame.y + offset + (line - 24.0) / 2.0).round();
        let handle = frame(self.side - 4.0 - 20.0, top, 20.0, 24.0);
        Some((
            Frame {
                x: handle.x - 21.0,
                ..handle
            },
            handle,
        ))
    }

    /// Hit-tests the point (`x`, `y`) on the page.
    pub fn hit(&self, x: f64, y: f64) -> Hit {
        let slop = |frame: Frame, dx: f64, dy: f64| Frame {
            x: frame.x - dx,
            y: frame.y - dy,
            width: frame.width + dx * 2.0,
            height: frame.height + dy * 2.0,
        };
        let row = self.blocks.iter().position(|placed| {
            let frame = placed.frame;
            frame.y <= y
                && y < frame.y + frame.height
                && x >= self.side - 49.0
                && x <= frame.x + frame.width
        });
        let button = row
            .and_then(|row| self.gutter(row))
            .map_or(0, |(plus, handle)| {
                if contains(plus, x, y) {
                    1
                } else if contains(handle, x, y) {
                    2
                } else {
                    0
                }
            });
        let number = |found: Option<usize>| found.map_or(-1, |found| found as i64);
        let below = y > self
            .slots
            .last()
            .map_or(0.0, |slot| slot.frame.y + slot.frame.height);
        Hit {
            row: number(row),
            button,
            block: number(
                self.blocks
                    .iter()
                    .position(|placed| contains(placed.frame, x, y)),
            ),
            todo: number(
                self.todos
                    .iter()
                    .find(|(_, marker)| contains(slop(*marker, 4.0, 4.0), x, y))
                    .map(|(block, _)| *block),
            ),
            slot: number(
                self.slots
                    .iter()
                    .position(|slot| contains(slop(slot.frame, 2.0, 2.0), x, y)),
            ),
            cell: number(self.slots.iter().position(|slot| {
                slot.cell.is_some() && contains(slop(slot.frame, 8.0, 6.0), x, y)
            })),
            media: number(
                self.media
                    .iter()
                    .find(|(_, figure)| contains(*figure, x, y))
                    .map(|(block, _)| *block),
            ),
            player: number(
                self.players
                    .iter()
                    .find(|(block, _)| {
                        self.block_frame(*block)
                            .is_some_and(|frame| contains(frame, x, y))
                    })
                    .map(|(block, _)| *block),
            ),
            below,
        }
    }

    /// Where dragged blocks `moving` would land with the pointer at `y`: the block under it and
    /// whether in its lower half. `None` over the dragged blocks.
    pub fn drop_at(&self, y: f64, moving: (usize, usize)) -> Option<(usize, bool)> {
        let over = self
            .blocks
            .iter()
            .position(|placed| placed.frame.y <= y && y < placed.frame.y + placed.frame.height)?;
        if (moving.0..=moving.1).contains(&over) {
            return None;
        }
        let frame = self.blocks[over].frame;
        Some((over, y >= frame.y + frame.height / 2.0))
    }

    /// The marks drawn under the blocks (selected rows) and over them (rings, the gutter and the
    /// drop indicator) for `state`.
    pub fn overlay(&self, state: &Overlay) -> (Vec<Mark>, Vec<Mark>) {
        let mut over = self.rings(state);
        over.extend(self.hovered_gutter(state));
        over.extend(self.drop_bar(state));
        (self.selected_rows(state), over)
    }

    /// Selected whole blocks get the accent at 22% across their row.
    fn selected_rows(&self, state: &Overlay) -> Vec<Mark> {
        let Some((first, last)) = state.chosen else {
            return Vec::new();
        };
        (first..=last)
            .filter_map(|block| self.block_frame(block))
            .map(|placed| Mark::Fill {
                frame: frame(
                    self.side,
                    placed.y,
                    state.width - 2.0 * self.side,
                    placed.height,
                ),
                radius: RADIUS,
                ink: accent(0.22),
            })
            .collect()
    }

    /// A selected or keyboard-focused media block gets a 2 pt ring in the accent at 60%, as does a
    /// selected player drawn as a player.
    fn rings(&self, state: &Overlay) -> Vec<Mark> {
        let ring = |frame: Frame, radius: f64| Mark::Stroke {
            frame: inset(frame, -1.0),
            radius,
            ink: accent(0.6),
            width: 2.0,
        };
        let media = self
            .media
            .iter()
            .filter(|(block, _)| state.chooses(*block) || state.ringed == Some(*block))
            .map(|(_, figure)| ring(*figure, 7.0));
        let players = self
            .players
            .iter()
            .filter(|(block, placed)| state.chooses(*block) && placed.drawn)
            .map(|(_, placed)| ring(placed.frame, 0.0));
        media.chain(players).collect()
    }

    /// The gutter of the hovered block while nothing is dragged: muted glyphs, with the hovered one
    /// on 8% of the text colour.
    fn hovered_gutter(&self, state: &Overlay) -> Vec<Mark> {
        let (None, Some((plus, handle))) =
            (state.drop, state.hovered.and_then(|row| self.gutter(row)))
        else {
            return Vec::new();
        };
        let mut marks = Vec::new();
        for (index, (button, icon)) in [(plus, Glyph::Plus), (handle, Glyph::DotsSixVertical)]
            .into_iter()
            .enumerate()
        {
            let lit = state.button == index as u32 + 1;
            if lit {
                marks.push(fill(button, 4.0, Token::Foreground, 0.08));
            }
            marks.push(Mark::Glyph {
                frame: frame(
                    button.x + button.width / 2.0 - 7.0,
                    button.y + button.height / 2.0 - 7.0,
                    14.0,
                    14.0,
                ),
                icon,
                color: if lit {
                    Paint::Theme(Token::Foreground)
                } else {
                    Paint::Theme(Token::MutedForeground)
                },
            });
        }
        marks
    }

    /// The drop indicator: a 3 pt bar in the accent at 60% along the row's edge.
    fn drop_bar(&self, state: &Overlay) -> Option<Mark> {
        let (block, after) = state.drop?;
        let placed = self.block_frame(block)?;
        let y = if after {
            placed.y + placed.height - 1.0
        } else {
            placed.y - 2.0
        };
        Some(Mark::Fill {
            frame: frame(self.side, y, placed.width - 12.0, 3.0),
            radius: 1.5,
            ink: accent(0.6),
        })
    }
}

impl Overlay {
    /// Whether whole block `block` is selected.
    fn chooses(&self, block: usize) -> bool {
        self.chosen
            .is_some_and(|(first, last)| (first..=last).contains(&block))
    }
}

/// The accent at `opacity`.
fn accent(opacity: f64) -> Ink {
    Ink {
        paint: Token::Primary.into(),
        opacity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::card::Measured;
    use crate::page::Page;

    /// Measures each character as half the font size wide, breaking anywhere and at newlines.
    fn fixed(words: &Words) -> Measured {
        let mut lines = 0;
        let mut widest: f64 = 0.0;
        let text: String = words
            .runs
            .iter()
            .map(|run| match run {
                Run::Words { text, .. } => text.clone(),
                Run::Space { .. } => String::new(),
            })
            .collect();
        let size = words
            .runs
            .iter()
            .find_map(|run| match run {
                Run::Words { style, .. } => Some(style.font.size),
                Run::Space { .. } => None,
            })
            .unwrap_or(0.0);
        if text.is_empty() || words.width <= 0.0 {
            return Measured {
                lines: 0,
                widest: 0.0,
            };
        }
        for line in text.split('\n') {
            let width = line.chars().count() as f64 * size / 2.0;
            let (count, shown) = if words.wraps {
                (
                    (width / words.width).ceil().max(1.0) as usize,
                    width.min(words.width),
                )
            } else {
                (1, width)
            };
            lines += count;
            widest = widest.max(shown);
        }
        Measured { lines, widest }
    }

    fn page(text: &str) -> Page {
        let mut next = 0;
        Page::from_file("Page", text, "en", &mut || {
            next += 1;
            format!("b{next}")
        })
    }

    const ROOM: Room = Room {
        width: 1000.0,
        viewport: 800.0,
        window_width: 1280.0,
    };

    fn layout_with(
        page: &Page,
        read_only: bool,
        shows: &mut ShowImage,
        players: &mut PlacePlayer,
    ) -> Layout {
        let sheet = Sheet {
            title: &page.title,
            blocks: &page.blocks,
            segments: &page.segments,
            numbers: &page.numbers,
            read_only,
            language: "en",
        };
        lay_out(&sheet, ROOM, &mut fixed, shows, players)
    }

    fn layout(text: &str) -> Layout {
        let page = page(text);
        layout_with(
            &page,
            false,
            &mut |_, _| ImageShows::Missing(String::new()),
            &mut |_, top, side, _| PlayerPlaced {
                end: top + 54.0,
                frame: frame(side, top, 300.0, 54.0),
                drawn: true,
            },
        )
    }

    fn words(layout: &Layout) -> Vec<&Words> {
        layout
            .marks
            .iter()
            .filter_map(|(mark, _, _)| {
                if let Mark::PageWords(words) = mark {
                    Some(words)
                } else {
                    None
                }
            })
            .collect()
    }

    #[test]
    fn the_title_and_a_paragraph_stack_from_the_page_margins() {
        let laid = layout("# Notes\n\nFirst words\n");
        // 6% of 1000 is 60; the title is one 38.4 pt line, rounded up, then 12 pt.
        assert_eq!(laid.side, 60.0);
        assert_eq!(laid.slots[0].block, -1);
        assert_eq!(laid.slots[0].frame, frame(60.0, 40.0, 880.0, 39.0));
        let title = words(&laid)[0];
        let Run::Words { style, .. } = &title.runs[0] else {
            panic!("the title")
        };
        assert_eq!(
            (style.font.weight, style.tracking, title.line_font),
            (700.0, -0.32, Font::sans(32.0, Font::REGULAR))
        );
        // The paragraph's text sits 1 pt down and 2 pt in from its block, which starts 2 pt above.
        let top = 40.0 + 39.0 + 12.0;
        assert_eq!(laid.slots[1].frame, frame(62.0, top + 3.0, 876.0, 24.0));
        assert_eq!(
            laid.blocks[0].frame,
            frame(54.0, top, 892.0, 2.0 + 24.0 + 2.0 + 2.0)
        );
        assert_eq!(laid.height, 800.0);
        // The margins stay within 56–96 pt.
        let narrow = lay_out(
            &Sheet {
                title: "",
                blocks: &[],
                segments: &[],
                numbers: &[],
                read_only: false,
                language: "en",
            },
            Room {
                width: 500.0,
                ..ROOM
            },
            &mut fixed,
            &mut |_, _| ImageShows::Missing(String::new()),
            &mut |_, _, _, _| unreachable!(),
        );
        assert_eq!((narrow.side, narrow.slots[0].frame.height), (56.0, 0.0));
    }

    #[test]
    fn a_heading_has_room_above_unless_it_opens_the_page_and_lines_of_its_own_size() {
        let laid = layout("# T\n\n## Opening\n\nWords\n\n### Later\n");
        let first = laid.blocks[0];
        // First: 2 pt above, less the 2 pt a heading drops; one 28.6 pt line (29 rounded) and 2 pt.
        assert_eq!(laid.slots[1].frame.y, first.frame.y + 1.0);
        assert_eq!(laid.slots[1].line_height, 29.0);
        assert!((first.frame.height - (28.6 + 2.0 + 2.0)).abs() < 1e-9);
        let later = laid.blocks[2];
        assert_eq!(laid.slots[3].frame.y, later.frame.y + 2.0 + 12.0 + 1.0);
        assert_eq!(laid.slots[3].font, Font::sans(18.0, Font::SEMIBOLD));
        assert_eq!(
            (later.kind, later.level, later.first, first.first),
            (1, 3, false, true)
        );
    }

    #[test]
    fn list_items_set_their_marker_in_its_room_and_their_words_after_it() {
        let laid = layout("# T\n\n- one\n  - two\n1. first\n- [ ] open\n- [x] done\n");
        let lines: Vec<(String, f64, f64)> = laid
            .marks
            .iter()
            .filter_map(|(mark, _, _)| match mark {
                Mark::Line {
                    text, x, max_width, ..
                } => Some((text.clone(), *x, *max_width)),
                _ => None,
            })
            .collect();
        // A 7.5 pt bullet centred in the 24 pt marker; the next level 24 pt in.
        assert_eq!(lines[0], ("•".to_owned(), 60.0 + 12.0 - 3.75, 24.0));
        assert_eq!(lines[1], ("◦".to_owned(), 60.0 + 24.0 + 12.0 - 3.75, 24.0));
        assert_eq!(lines[2], ("1.".to_owned(), 60.0 + 12.0 - 7.5, 40.0));
        assert_eq!(laid.slots[1].frame.x, 60.0 + 28.0 + 2.0);
        assert_eq!(laid.slots[1].frame.width, 1000.0 - 60.0 - 88.0 - 4.0);
        assert_eq!(laid.slots[2].frame.x, 60.0 + 24.0 + 28.0 + 2.0);
        // To-do markers take presses; an open one is a ring, a done one a tick.
        assert_eq!(
            laid.todos
                .iter()
                .map(|(block, _)| *block)
                .collect::<Vec<_>>(),
            [3, 4]
        );
        let marker = laid.todos[0].1;
        assert!(laid.marks.iter().any(|(mark, _, _)| matches!(mark,
            Mark::Stroke { frame: ring, radius, width, .. } if *ring == frame(marker.x + 4.75, marker.y + 5.75, 14.5, 14.5) && *radius == 5.25 && *width == 1.5)));
        assert!(laid.marks.iter().any(|(mark, _, _)| matches!(
            mark,
            Mark::Glyph {
                icon: Glyph::Check,
                color: Paint::Theme(Token::Background),
                ..
            }
        )));
        let done = words(&laid).into_iter().last().expect("the done words");
        let Run::Words { style, .. } = &done.runs[0] else {
            panic!("words")
        };
        assert_eq!((style.color, style.strike), (Token::MutedForeground, true));
    }

    #[test]
    fn quotes_code_dividers_and_tables_draw_their_rules_and_grounds() {
        let laid = layout("# T\n\n> said\n\n```\nlet a = 1;\nlet b = 2;\n```\n\n---\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n");
        let quote = laid.blocks[0].frame;
        assert!(laid.marks.iter().any(|(mark, _, _)| *mark
            == fill(
                frame(60.0, quote.y + 2.0, 3.0, 26.0),
                0.0,
                Token::Foreground,
                1.0
            )));
        assert_eq!(laid.slots[1].frame.x, 60.0 + 17.0);
        assert_eq!(laid.slots[1].frame.width, 880.0 - 21.0);
        // Code: two 20.15 pt lines on a background 12 pt above and below, text 16 pt in.
        let code = laid.blocks[1].frame;
        let slot = &laid.slots[2];
        assert_eq!(
            (slot.frame.x, slot.frame.y, slot.font, slot.ground),
            (76.0, code.y + 14.0, Font::mono(13.0), Some(Token::Muted))
        );
        assert!((slot.frame.height - 40.3).abs() < 1e-9);
        assert!((code.height - (2.0 + 40.3 + 24.0 + 2.0)).abs() < 1e-9);
        let divider = laid.blocks[2].frame;
        assert_eq!(divider.height, 21.0);
        assert!(laid.marks.iter().any(|(mark, _, _)| *mark
            == fill(
                frame(60.0, divider.y + 10.0, 880.0, 1.0),
                0.0,
                Token::Border,
                1.0
            )));
        // The table: cells share the column less 24 pt, rows are 34 pt, and text sits 8 pt in, 6 pt
        // down.
        let table = laid.blocks[3].frame;
        let cells: Vec<&Slot> = laid
            .slots
            .iter()
            .filter(|slot| slot.cell.is_some())
            .collect();
        assert_eq!(cells.len(), 4);
        assert_eq!(
            cells[1].frame,
            frame(60.0 + 428.0 + 8.0, table.y + 4.0 + 6.0, 412.0, 21.0)
        );
        assert_eq!((cells[0].font.weight, cells[2].font.weight), (600.0, 400.0));
        assert!((table.height - (4.0 + 68.0 + 31.0 - 2.0 + 2.0)).abs() < 1e-9);
        // A cell shows its text while not being edited, and its 8% background while it is.
        let slot = laid
            .slots
            .iter()
            .position(|slot| slot.cell == Some((1, 0)))
            .expect("a cell");
        assert!(laid
            .marks
            .iter()
            .any(|(mark, shown, _)| *shown == Shown::Resting(slot)
                && matches!(mark, Mark::PageWords(_))));
        assert!(laid
            .marks
            .iter()
            .any(|(mark, shown, _)| *shown == Shown::Typing(slot)
                && matches!(mark, Mark::Fill { ink, .. } if ink.opacity == 0.08)));
    }

    #[test]
    fn a_long_cell_wraps_and_its_whole_row_grows_to_fit_it() {
        // 412 pt at 7 pt per character fits 59 per line, so 130 characters take 3 lines.
        let long = "x".repeat(130);
        let laid = layout(&format!(
            "# T\n\n| a | b |\n| --- | --- |\n| {long} | 2 |\n| 3 | 4 |\n"
        ));
        let table = laid.blocks[0].frame;
        let cell = |at: (usize, usize)| {
            laid.slots
                .iter()
                .find(|slot| slot.cell == Some(at))
                .expect("a cell")
                .frame
        };
        // The header row is one line; the long cell's row is three, and its neighbour matches.
        assert_eq!(cell((1, 0)).y, table.y + 4.0 + 34.0 + 6.0);
        assert_eq!(cell((1, 0)).height, 63.0);
        assert_eq!(cell((1, 1)).height, 63.0);
        assert_eq!(cell((2, 0)).y, cell((1, 0)).y + 63.0 + 13.0);
        assert!((table.height - (4.0 + 34.0 + 76.0 + 34.0 + 31.0 - 2.0 + 2.0)).abs() < 1e-9);
        // At rest the text wraps in the cell as the field does.
        let resting = laid
            .slots
            .iter()
            .position(|slot| slot.cell == Some((1, 0)))
            .expect("a cell");
        assert!(laid
            .marks
            .iter()
            .any(|(mark, shown, _)| *shown == Shown::Resting(resting)
                && matches!(mark, Mark::PageWords(words) if words.wraps && words.width == 412.0)));
    }

    #[test]
    fn a_media_block_is_a_frame_and_a_caption_with_its_placeholder() {
        let laid = layout("# T\n\n![](asset:a)\n\n![A shot](asset:b)\n");
        assert_eq!(laid.media.len(), 2);
        let (block, figure) = laid.media[0];
        assert_eq!(
            (block, figure),
            (0, frame(60.0, laid.blocks[0].frame.y + 6.0, 880.0, 67.5))
        );
        assert!((laid.blocks[0].frame.height - (6.0 + 67.5 + 24.75 + 6.0)).abs() < 1e-9);
        let caption = laid
            .slots
            .iter()
            .position(|slot| slot.caption)
            .expect("a caption");
        let text_of = |shown: Shown| {
            laid.marks.iter().find_map(|(mark, at, _)| match mark {
                Mark::Words(words) if *at == shown => match &words.runs[0] {
                    Run::Words { text, style } => Some((text.clone(), style.opacity)),
                    Run::Space { .. } => None,
                },
                _ => None,
            })
        };
        assert_eq!(
            text_of(Shown::Resting(caption)),
            Some(("Add a caption".to_owned(), 0.5))
        );
        assert_eq!(
            text_of(Shown::TypingEmpty(caption)),
            Some(("Add a caption".to_owned(), 0.5))
        );
        assert_eq!(
            text_of(Shown::Resting(caption + 1)),
            Some(("A shot".to_owned(), 1.0))
        );
    }

    #[test]
    fn a_player_is_placed_by_the_shell_with_room_around_it() {
        let laid = layout("# T\n\n<video src=\"clip.mp4\"></video>\n\nafter\n");
        let (block, placed) = laid.players[0];
        assert_eq!((block, placed.frame.y), (0, laid.blocks[0].frame.y + 6.0));
        assert_eq!(laid.blocks[0].frame.height, 6.0 + 54.0 + 6.0);
        assert_eq!(laid.blocks[0].kind, 11);
    }

    #[test]
    fn an_image_is_a_block_of_its_line_and_the_block_goes_on_under_a_last_one() {
        let page = page("# T\n\nbefore ![shot](a.png) after\n\n![gone](b.png)\n");
        let laid = layout_with(
            &page,
            false,
            &mut |block, _| match block {
                0 => ImageShows::Picture {
                    id: "a".into(),
                    size: Some((2000, 400)),
                },
                _ => ImageShows::Missing("gone".into()),
            },
            &mut |_, _, _, _| unreachable!(),
        );
        let top = laid.slots[1].frame.y;
        // "before " on its line, the picture shrunk to the line's 876 pt on a line 12 pt taller,
        // then " after".
        let picture = laid.marks.iter().find_map(|(mark, _, _)| match mark {
            Mark::Image { frame, id, .. } if id == "a" => Some(*frame),
            _ => None,
        });
        assert_eq!(picture, Some(frame(62.0, top + 24.0 + 6.0, 876.0, 175.2)));
        let all = words(&laid);
        assert_eq!(all[2].top, top + 24.0 + 175.2 + 12.0);
        assert_eq!(laid.slots[1].frame.height, (24.0 + 187.2 + 24.0_f64).ceil());
        // A missing file: its alt text, small and muted on a line 12 pt taller; and after a
        // trailing image, an empty line.
        let gone = all[3];
        assert_eq!(
            (gone.line_height, gone.line_font),
            (20.8 + 12.0, Font::sans(13.0, Font::REGULAR))
        );
        assert_eq!(laid.slots[2].frame.height, (32.8 + 24.0_f64).ceil());
    }

    #[test]
    fn inline_code_has_no_room_before_it_at_the_start_of_a_line() {
        let laid = layout("# T\n\n`code` then `more`\n");
        let runs = &words(&laid)[1].runs;
        assert!(matches!(runs[0], Run::Words { ref text, .. } if text == "code"));
        assert!(matches!(runs[1], Run::Space { width, .. } if width == 4.8));
        assert!(matches!(runs[3], Run::Space { .. }));
        // The page keeps a paragraph's line breaks.
        let laid = layout("# T\n\nfirst\nsecond\n");
        let Run::Words { text, .. } = &words(&laid)[1].runs[0] else {
            panic!("words")
        };
        assert_eq!(text, "first\nsecond");
    }

    #[test]
    fn a_page_that_cannot_be_typed_in_is_a_document() {
        let page = page("Line one\n\n<video src=\"a.mp4\"></video>\n");
        let laid = layout_with(
            &page,
            true,
            &mut |_, _| unreachable!(),
            &mut |_, _, _, _| unreachable!(),
        );
        // 5% of the window, at most 64 pt.
        assert_eq!(laid.side, 64.0);
        assert!(laid.slots.is_empty() && laid.blocks.is_empty());
        let all = words(&laid);
        assert_eq!(
            (all[0].top, all[0].line_font),
            (48.0, Font::sans(32.0, Font::REGULAR))
        );
        // The title's 39 pt, 28 pt below it; each paragraph 24.75 pt rounded up, 13.6 pt apart.
        assert_eq!(all[1].top, 48.0 + 39.0 + 28.0);
        assert_eq!(all[2].top, 48.0 + 39.0 + 28.0 + 25.0 + 13.6);
        let Run::Words { text, .. } = &all[2].runs[0] else {
            panic!("words")
        };
        assert_eq!(text, "<video src=\"a.mp4\"></video>");
    }

    #[test]
    fn the_gutter_sits_on_each_kind_of_first_line() {
        let laid = layout("# T\n\n## Heading\n\nWords\n\n```\nx\n```\n\n---\n");
        let (plus, handle) = laid.gutter(0).expect("a gutter");
        let first = laid.blocks[0].frame;
        // A first heading: 1 pt above its 28.6 pt line.
        assert_eq!(
            handle,
            frame(60.0 - 24.0, (first.y + 1.0 + 2.3).round(), 20.0, 24.0)
        );
        assert_eq!(plus.x, handle.x - 21.0);
        assert_eq!(
            laid.gutter(1).expect("a gutter").1.y,
            (laid.blocks[1].frame.y + 3.0).round()
        );
        assert_eq!(
            laid.gutter(2).expect("a gutter").1.y,
            (laid.blocks[2].frame.y + 14.0 - 1.92).round()
        );
        assert_eq!(
            laid.gutter(3).expect("a gutter").1.y,
            (laid.blocks[3].frame.y - 1.5).round()
        );
        assert_eq!(laid.gutter(9), None);
    }

    #[test]
    fn a_point_is_on_a_row_its_gutter_its_words_or_below_them() {
        let laid = layout("# T\n\nWords\n\n- [ ] task\n\n| a |\n| --- |\n| 1 |\n");
        let row = laid.blocks[0].frame;
        let (plus, handle) = laid.gutter(0).expect("a gutter");
        let at_plus = laid.hit(plus.x + 2.0, plus.y + 2.0);
        assert_eq!((at_plus.row, at_plus.button, at_plus.block), (0, 1, -1));
        assert_eq!(laid.hit(handle.x + 2.0, handle.y + 2.0).button, 2);
        // Left of the gutter is no row.
        assert_eq!(laid.hit(60.0 - 50.0, row.y + 5.0).row, -1);
        let words = laid.hit(100.0, row.y + 5.0);
        assert_eq!((words.row, words.block, words.slot), (0, 0, 1));
        let marker = laid.todos[0].1;
        assert_eq!(laid.hit(marker.x - 3.0, marker.y).todo, 1);
        let cell = laid
            .slots
            .iter()
            .position(|slot| slot.cell == Some((1, 0)))
            .expect("a cell") as i64;
        let cell_frame = laid.slots[cell as usize].frame;
        assert_eq!(laid.hit(cell_frame.x - 7.0, cell_frame.y + 2.0).cell, cell);
        assert!(laid.hit(100.0, 790.0).below);
        assert!(!laid.hit(100.0, row.y + 5.0).below);
        // A drag lands by the half of the block under it, never on the dragged blocks.
        let second = laid.blocks[1].frame;
        assert_eq!(laid.drop_at(second.y + 1.0, (0, 0)), Some((1, false)));
        assert_eq!(
            laid.drop_at(second.y + second.height - 1.0, (0, 0)),
            Some((1, true))
        );
        assert_eq!(laid.drop_at(row.y + 1.0, (0, 1)), None);
    }

    #[test]
    fn the_overlay_draws_selected_rows_rings_the_gutter_and_the_drop_bar() {
        let laid = layout("# T\n\nWords\n\n![](asset:a)\n\nMore\n");
        let state = Overlay {
            chosen: Some((0, 1)),
            hovered: Some(2),
            button: 2,
            drop: None,
            ringed: None,
            width: 1000.0,
        };
        let (under, over) = laid.overlay(&state);
        let row = laid.blocks[0].frame;
        assert_eq!(
            under[0],
            Mark::Fill {
                frame: frame(60.0, row.y, 880.0, row.height),
                radius: 6.0,
                ink: Ink {
                    paint: Token::Primary.into(),
                    opacity: 0.22
                }
            }
        );
        assert_eq!(under.len(), 2);
        // The selected media block has its ring; the handle is highlighted.
        assert!(
            matches!(over[0], Mark::Stroke { radius, width, .. } if radius == 7.0 && width == 2.0)
        );
        let (_, handle) = laid.gutter(2).expect("a gutter");
        assert_eq!(
            over[1],
            Mark::Glyph {
                frame: frame(handle.x - 21.0 + 3.0, handle.y + 5.0, 14.0, 14.0),
                icon: Glyph::Plus,
                color: Paint::Theme(Token::MutedForeground)
            }
        );
        assert_eq!(over[2], fill(handle, 4.0, Token::Foreground, 0.08));
        assert!(matches!(
            over[3],
            Mark::Glyph {
                icon: Glyph::DotsSixVertical,
                color: Paint::Theme(Token::Foreground),
                ..
            }
        ));
        // While dragging: no gutter, and the bar under the block it lands after.
        let (_, over) = laid.overlay(&Overlay {
            drop: Some((2, true)),
            chosen: None,
            ..state
        });
        let last = laid.blocks[2].frame;
        assert_eq!(
            over,
            [Mark::Fill {
                frame: frame(60.0, last.y + last.height - 1.0, 880.0, 3.0),
                radius: 1.5,
                ink: Ink {
                    paint: Token::Primary.into(),
                    opacity: 0.6
                }
            }]
        );
    }
}
