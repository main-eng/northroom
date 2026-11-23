//! What the shell draws for a terminal, in the order it draws it: cell backgrounds, runs of
//! text, underlines and strikethroughs, the input method's composing text, then the cursor
//! (ADR-0001). The shell sets the text in the terminal font with its own text system, one run
//! per piece with every glyph on its cell; everything else here is a filled rectangle.
//!
//! Positions are the view's pixels from its top left, on a grid of whole-pixel cells measured
//! from the shell's font (`CellMetrics`).

use super::cells::{Cell, Cursor, CursorShape, Rgb, Underline};
use super::palette::Palette;
use std::ops::Range;

/// The terminal's text size, in points.
pub const FONT_POINTS: f32 = 13.0;
/// 13 pt text on 15 pt lines.
const LINE_HEIGHT_RATIO: f32 = 15.0 / 13.0;
/// How strongly dim text is drawn.
const DIM_ALPHA: f32 = 0.6;
/// How far above the baseline a strikethrough sits, as a share of the line.
const STRIKEOUT_RAISE: f32 = 0.3;

/// The terminal font's metrics at `FONT_POINTS`, in pixels, as the shell's text system measures
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontMetrics {
    pub ascent: f32,
    pub descent: f32,
    /// The pen advance of one character.
    pub advance: f32,
}

/// The grid's metrics in whole pixels, so every cell starts on a pixel and text stays sharp.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
    /// From a cell's top to the text's baseline.
    pub baseline: f32,
    /// Underline, strikethrough and cursor thickness.
    pub stroke: f32,
}

impl CellMetrics {
    /// The cell for `font` at `scale` pixels per point. The font's height is centred in the
    /// line the way the rest of the app centres text (`baseline::browser`): ascent and descent
    /// rounded to whole pixels, with the smaller half of the leftover space on top.
    pub fn of_font(font: FontMetrics, scale: f32) -> Self {
        let height = (FONT_POINTS * scale * LINE_HEIGHT_RATIO).round().max(1.0);
        let (ascent, descent) = (font.ascent.round(), font.descent.round());
        Self {
            width: font.advance.round().max(1.0),
            height,
            baseline: ((height - (ascent + descent)) / 2.0).floor() + ascent,
            stroke: scale.round().max(1.0),
        }
    }
}

/// A colour to draw in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ink {
    pub rgb: Rgb,
    pub alpha: f32,
}

impl Ink {
    fn solid(rgb: Rgb) -> Self {
        Self { rgb, alpha: 1.0 }
    }
}

/// Which face of the terminal font a run is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Face {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Face {
    pub fn of(bold: bool, italic: bool) -> Self {
        match (bold, italic) {
            (false, false) => Self::Regular,
            (true, false) => Self::Bold,
            (false, true) => Self::Italic,
            (true, true) => Self::BoldItalic,
        }
    }

    pub fn is_bold(self) -> bool {
        matches!(self, Self::Bold | Self::BoldItalic)
    }

    pub fn is_italic(self) -> bool {
        matches!(self, Self::Italic | Self::BoldItalic)
    }
}

/// One thing to draw.
#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    /// A rectangle: x, y, width, height.
    Fill { rect: [f32; 4], ink: Ink },
    /// Text from `origin`, a cell's top left, one character a cell: `text` is its bytes in the
    /// frame's text. A wide character is always a run of its own.
    Text {
        origin: [f32; 2],
        cells: usize,
        text: Range<usize>,
        ink: Ink,
        face: Face,
    },
}

/// A terminal's picture, as pieces to draw over its background.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub background: Rgb,
    pub pieces: Vec<Piece>,
    /// Every run's text, one after another.
    pub text: String,
    /// Where the cursor is: x, y, width, height.
    pub cursor_rect: [f32; 4],
    /// A cursor blinks, so the frame changes when it turns.
    pub blinking: bool,
}

/// What a frame shows besides the emulator's cells.
pub struct View<'a> {
    pub palette: Palette,
    pub cell: CellMetrics,
    pub focused: bool,
    /// A blinking cursor is in the visible half of its blink.
    pub blink_on: bool,
    /// Text an input method is still composing, drawn at the cursor.
    pub composing: &'a str,
}

/// How the cursor is drawn over its cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CursorMark {
    Nothing,
    Bar,
    Outline,
    Underline,
}

impl View<'_> {
    fn is_composing(&self) -> bool {
        !self.composing.is_empty()
    }

    /// A bar where text is being composed; an outline when unfocused (or for a hollow block);
    /// otherwise the program's shape while the blink is `shown`. A focused block inverts its cell
    /// instead, so it needs no mark of its own.
    fn cursor_mark(&self, shape: CursorShape, shown: bool) -> CursorMark {
        match shape {
            CursorShape::Hidden => CursorMark::Nothing,
            _ if self.is_composing() => CursorMark::Bar,
            CursorShape::HollowBlock => CursorMark::Outline,
            _ if !self.focused => CursorMark::Outline,
            _ if !shown => CursorMark::Nothing,
            CursorShape::Beam => CursorMark::Bar,
            CursorShape::Underline => CursorMark::Underline,
            CursorShape::Block => CursorMark::Nothing,
        }
    }
}

/// The frame of the visible `cells` (top to bottom, left to right) with `cursor`.
pub fn build(cells: &mut dyn Iterator<Item = Cell<'_>>, cursor: Cursor, view: &View<'_>) -> Frame {
    let blinking = view.focused && cursor.blinking && cursor.shape != CursorShape::Hidden;
    let shown = !blinking || view.blink_on;
    // A focused block cursor draws its cell with colours inverted.
    let block_at =
        (view.focused && shown && !view.is_composing() && cursor.shape == CursorShape::Block)
            .then_some((cursor.line, cursor.column));
    let mut painter = Painter::new(view, block_at);
    for cell in cells {
        painter.paint_cell(&cell);
    }
    painter.flush();
    let mut pieces = painter.grounds;
    pieces.append(&mut painter.texts);
    pieces.append(&mut painter.lines);
    let mut over = Overlay {
        view,
        pieces,
        text: painter.text,
    };
    let (x, y) = origin(view.cell, cursor.line, cursor.column);
    let cursor_rect = [x, y, view.cell.width, view.cell.height];
    let ink = Ink::solid(view.palette.foreground);
    let x = over.paint_composing(x, y, ink);
    // The cursor covers both cells of a wide character, unless text is being composed there.
    let wide = cursor.wide && !view.is_composing();
    let rect = [x, y, span(view.cell, wide), view.cell.height];
    over.paint_cursor(view.cursor_mark(cursor.shape, shown), rect, ink);
    Frame {
        background: view.palette.background,
        pieces: over.pieces,
        text: over.text,
        cursor_rect,
        blinking,
    }
}

/// A cell's top left.
fn origin(cell: CellMetrics, line: i32, column: usize) -> (f32, f32) {
    (column as f32 * cell.width, line as f32 * cell.height)
}

/// A character's drawn width: two cells when `wide`.
fn span(cell: CellMetrics, wide: bool) -> f32 {
    cell.width * if wide { 2.0 } else { 1.0 }
}

/// Text being gathered into one run.
struct Run {
    origin: [f32; 2],
    line: usize,
    /// The column the next character would take.
    next_column: usize,
    cells: usize,
    /// Where its text starts in the frame's text.
    start: usize,
    ink: Ink,
    face: Face,
    /// Spaces at its end, dropped if nothing follows them.
    trailing_spaces: usize,
}

impl Run {
    /// Whether a cell at `line`, `column` in `ink` and `face` continues the run. A blank
    /// continues any run it touches, whatever its colours.
    fn takes(&self, line: usize, column: usize, blank: bool, ink: Ink, face: Face) -> bool {
        self.line == line
            && self.next_column == column
            && (blank || (self.ink == ink && self.face == face))
    }

    /// Adds `cell`'s character and marks to the run, whose text ends `text`.
    fn push(&mut self, text: &mut String, cell: &Cell<'_>, blank: bool) {
        text.push(cell.character);
        text.extend(cell.marks);
        self.cells += 1;
        self.next_column += if cell.wide { 2 } else { 1 };
        self.trailing_spaces = if blank { self.trailing_spaces + 1 } else { 0 };
    }
}

/// Gathers the grid's backgrounds, text runs and lines.
struct Painter<'a> {
    view: &'a View<'a>,
    block_at: Option<(i32, usize)>,
    grounds: Vec<Piece>,
    texts: Vec<Piece>,
    lines: Vec<Piece>,
    text: String,
    run: Option<Run>,
}

impl<'a> Painter<'a> {
    fn new(view: &'a View<'a>, block_at: Option<(i32, usize)>) -> Self {
        Self {
            view,
            block_at,
            grounds: Vec::new(),
            texts: Vec::new(),
            lines: Vec::new(),
            text: String::new(),
            run: None,
        }
    }

    /// One grid cell: its background, text and lines.
    fn paint_cell(&mut self, cell: &Cell<'_>) {
        let (x, y) = origin(self.view.cell, cell.line as i32, cell.column);
        let width = span(self.view.cell, cell.wide);
        let (foreground, background) = self.cell_colours(cell);
        if let Some(background) = background {
            self.paint_ground([x, y, width, self.view.cell.height], background);
        }
        if cell.style.hidden {
            self.flush();
            return;
        }
        let alpha = if cell.style.dim { DIM_ALPHA } else { 1.0 };
        let ink = Ink {
            rgb: foreground.unwrap_or(self.view.palette.foreground),
            alpha,
        };
        self.add_text(cell, [x, y], ink);
        self.paint_lines(cell, x, y, width, ink);
    }

    /// A cell's text and background colours, `None` for the terminal's own. Inverse video swaps
    /// them, the selection has its own background, and a block cursor inverts its cell.
    fn cell_colours(&self, cell: &Cell<'_>) -> (Option<Rgb>, Option<Rgb>) {
        let palette = self.view.palette;
        let (mut foreground, mut background) = (
            palette.resolve(cell.foreground, cell.style.bold),
            palette.resolve(cell.background, false),
        );
        if cell.style.inverse {
            (foreground, background) = (
                Some(background.unwrap_or(palette.background)),
                Some(foreground.unwrap_or(palette.foreground)),
            );
        }
        if cell.selected {
            background = Some(palette.selection);
        }
        if self.block_at == Some((cell.line as i32, cell.column)) {
            (foreground, background) = (
                Some(background.unwrap_or(palette.background)),
                Some(palette.foreground),
            );
        }
        (foreground, background)
    }

    /// A background, joined to the one before when it continues it in the same colour.
    fn paint_ground(&mut self, rect: [f32; 4], rgb: Rgb) {
        let ink = Ink::solid(rgb);
        if let Some(Piece::Fill {
            rect: last,
            ink: last_ink,
        }) = self.grounds.last_mut()
        {
            if *last_ink == ink && last[1] == rect[1] && last[0] + last[2] == rect[0] {
                last[2] += rect[2];
                return;
            }
        }
        self.grounds.push(Piece::Fill { rect, ink });
    }

    /// Adds a cell's character to the run it continues, or starts a run. A wide character is a
    /// run of its own, because the shell sets one glyph a cell.
    fn add_text(&mut self, cell: &Cell<'_>, origin: [f32; 2], ink: Ink) {
        let blank = cell.character == ' ' && cell.marks.is_empty();
        let face = Face::of(cell.style.bold, cell.style.italic);
        let continues = !cell.wide
            && self
                .run
                .as_ref()
                .is_some_and(|run| run.takes(cell.line, cell.column, blank, ink, face));
        if !continues {
            self.flush();
            if blank {
                return;
            }
            self.run = Some(Run {
                origin,
                line: cell.line,
                next_column: cell.column,
                cells: 0,
                start: self.text.len(),
                ink,
                face,
                trailing_spaces: 0,
            });
        }
        if let Some(run) = self.run.as_mut() {
            run.push(&mut self.text, cell, blank);
        }
        if cell.wide {
            self.flush();
        }
    }

    /// Ends the run being gathered, without its trailing spaces.
    fn flush(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        self.text.truncate(self.text.len() - run.trailing_spaces);
        let cells = run.cells - run.trailing_spaces;
        if cells == 0 {
            return;
        }
        self.texts.push(Piece::Text {
            origin: run.origin,
            cells,
            text: run.start..self.text.len(),
            ink: run.ink,
            face: run.face,
        });
    }

    /// A cell's underlines and strikethrough, `width` wide from origin `x`, `y`, at the alpha of
    /// its text `ink`.
    fn paint_lines(&mut self, cell: &Cell<'_>, x: f32, y: f32, width: f32, ink: Ink) {
        let metrics = self.view.cell;
        let stroke = metrics.stroke;
        if cell.style.underline != Underline::None {
            let underline_ink = cell
                .underline_colour
                .and_then(|colour| self.view.palette.resolve(colour, false))
                .map_or(ink, |rgb| Ink {
                    rgb,
                    alpha: ink.alpha,
                });
            let under = y + metrics.baseline + stroke;
            self.paint_line([x, under, width, stroke], underline_ink);
            if cell.style.underline == Underline::Double {
                self.paint_line([x, under + stroke * 2.0, width, stroke], underline_ink);
            }
        }
        if cell.style.strikeout {
            let through = y + metrics.baseline - (metrics.height * STRIKEOUT_RAISE).round();
            self.paint_line([x, through, width, stroke], ink);
        }
    }

    /// A line, joined to the one before when it continues it in the same colour.
    fn paint_line(&mut self, rect: [f32; 4], ink: Ink) {
        if let Some(Piece::Fill {
            rect: last,
            ink: last_ink,
        }) = self.lines.last_mut()
        {
            if *last_ink == ink && last[1] == rect[1] && last[0] + last[2] == rect[0] {
                last[2] += rect[2];
                return;
            }
        }
        self.lines.push(Piece::Fill { rect, ink });
    }
}

/// What is drawn over the grid: composing text and the cursor.
struct Overlay<'a> {
    view: &'a View<'a>,
    pieces: Vec<Piece>,
    text: String,
}

impl Overlay<'_> {
    /// Draws the input method's composing text at the cursor from `x`, `y`, underlined, until it
    /// is committed. Returns where it ends.
    fn paint_composing(&mut self, x: f32, y: f32, ink: Ink) -> f32 {
        let cell = self.view.cell;
        let ground = Ink::solid(self.view.palette.background);
        let mut x = x;
        for character in self.view.composing.chars() {
            let wide = unicode_width::UnicodeWidthChar::width(character) == Some(2);
            let width = span(cell, wide);
            self.fill([x, y, width, cell.height], ground);
            let start = self.text.len();
            self.text.push(character);
            self.pieces.push(Piece::Text {
                origin: [x, y],
                cells: 1,
                text: start..self.text.len(),
                ink,
                face: Face::Regular,
            });
            self.fill(
                [x, y + cell.baseline + cell.stroke, width, cell.stroke],
                ink,
            );
            x += width;
        }
        x
    }

    fn paint_cursor(&mut self, mark: CursorMark, rect: [f32; 4], ink: Ink) {
        let [x, y, width, height] = rect;
        let stroke = self.view.cell.stroke;
        match mark {
            CursorMark::Nothing => {}
            CursorMark::Bar => self.fill([x, y, stroke, height], ink),
            CursorMark::Outline => {
                self.fill([x, y, width, stroke], ink);
                self.fill([x, y + height - stroke, width, stroke], ink);
                self.fill([x, y + stroke, stroke, height - stroke * 2.0], ink);
                self.fill(
                    [
                        x + width - stroke,
                        y + stroke,
                        stroke,
                        height - stroke * 2.0,
                    ],
                    ink,
                );
            }
            CursorMark::Underline => {
                let thick = stroke * 2.0;
                self.fill([x, y + height - thick, width, thick], ink);
            }
        }
    }

    fn fill(&mut self, rect: [f32; 4], ink: Ink) {
        self.pieces.push(Piece::Fill { rect, ink });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::cells::{rgb, Colour, Style};

    const CELL: CellMetrics = CellMetrics {
        width: 8.0,
        height: 15.0,
        baseline: 11.0,
        stroke: 1.0,
    };

    fn palette() -> Palette {
        Palette::on([17, 17, 17], [238, 238, 240], [60, 60, 90])
    }

    fn cell(line: usize, column: usize, character: char) -> Cell<'static> {
        Cell {
            line,
            column,
            character,
            marks: &[],
            wide: false,
            foreground: Colour::Foreground,
            background: Colour::Background,
            underline_colour: None,
            style: Style::default(),
            selected: false,
        }
    }

    /// The cells of `lines`, one character a cell.
    fn cells_of(lines: &[&str]) -> Vec<Cell<'static>> {
        lines
            .iter()
            .enumerate()
            .flat_map(|(line, text)| {
                text.chars()
                    .enumerate()
                    .map(move |(column, character)| cell(line, column, character))
            })
            .collect()
    }

    fn view(focused: bool) -> View<'static> {
        View {
            palette: palette(),
            cell: CELL,
            focused,
            blink_on: true,
            composing: "",
        }
    }

    fn hidden_cursor() -> Cursor {
        Cursor {
            line: 0,
            column: 0,
            shape: CursorShape::Hidden,
            blinking: false,
            wide: false,
        }
    }

    fn texts(frame: &Frame) -> Vec<(&str, [f32; 2], usize)> {
        frame
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Text {
                    origin,
                    cells,
                    text,
                    ..
                } => Some((&frame.text[text.clone()], *origin, *cells)),
                Piece::Fill { .. } => None,
            })
            .collect()
    }

    #[test]
    fn the_cell_is_whole_pixels_from_the_fonts_metrics() {
        let font = FontMetrics {
            ascent: 12.2,
            descent: 3.1,
            advance: 7.83,
        };
        assert_eq!(
            CellMetrics::of_font(font, 1.0),
            CellMetrics {
                width: 8.0,
                height: 15.0,
                baseline: 12.0,
                stroke: 1.0,
            }
        );
        let font = FontMetrics {
            ascent: 24.4,
            descent: 6.2,
            advance: 15.66,
        };
        assert_eq!(CellMetrics::of_font(font, 2.0).height, 30.0);
        assert_eq!(CellMetrics::of_font(font, 2.0).stroke, 2.0);
    }

    #[test]
    fn a_line_of_text_is_one_run_without_its_trailing_spaces() {
        let cells = cells_of(&["ls -la   ", "", "  next"]);
        let frame = build(&mut cells.into_iter(), hidden_cursor(), &view(true));
        assert_eq!(
            texts(&frame),
            [("ls -la", [0.0, 0.0], 6), ("next", [16.0, 30.0], 4)]
        );
        // Nothing but text on the terminal's own background.
        assert!(frame
            .pieces
            .iter()
            .all(|piece| matches!(piece, Piece::Text { .. })));
        assert_eq!(frame.background, palette().background);
    }

    #[test]
    fn a_change_of_colour_or_face_starts_a_run_and_a_wide_character_stands_alone() {
        let mut cells = cells_of(&["ab日cd"]);
        cells[1].foreground = Colour::Indexed(1);
        // The wide character takes columns 2 and 3; the grid has no cell for its second half.
        cells[2].wide = true;
        for later in &mut cells[3..] {
            later.column += 1;
        }
        cells[4].style.bold = true;
        let frame = build(&mut cells.into_iter(), hidden_cursor(), &view(true));
        assert_eq!(
            texts(&frame),
            [
                ("a", [0.0, 0.0], 1),
                ("b", [8.0, 0.0], 1),
                ("日", [16.0, 0.0], 1),
                ("c", [32.0, 0.0], 1),
                ("d", [40.0, 0.0], 1),
            ]
        );
    }

    #[test]
    fn backgrounds_join_and_the_selection_and_inverse_change_them() {
        let mut cells = cells_of(&["abcd"]);
        cells[0].background = Colour::Indexed(4);
        cells[1].background = Colour::Indexed(4);
        cells[2].selected = true;
        cells[3].style.inverse = true;
        let frame = build(&mut cells.into_iter(), hidden_cursor(), &view(true));
        let fills: Vec<([f32; 4], Rgb)> = frame
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Fill { rect, ink } => Some((*rect, ink.rgb)),
                Piece::Text { .. } => None,
            })
            .collect();
        assert_eq!(
            fills,
            [
                ([0.0, 0.0, 16.0, 15.0], rgb(0x34, 0x65, 0xa4)),
                ([16.0, 0.0, 8.0, 15.0], palette().selection),
                ([24.0, 0.0, 8.0, 15.0], palette().foreground),
            ]
        );
    }

    #[test]
    fn a_focused_block_cursor_inverts_its_cell_and_an_unfocused_one_is_an_outline() {
        let cursor = Cursor {
            line: 0,
            column: 1,
            shape: CursorShape::Block,
            blinking: true,
            wide: false,
        };
        let frame = build(&mut cells_of(&["ab"]).into_iter(), cursor, &view(true));
        assert!(frame.blinking);
        assert_eq!(frame.cursor_rect, [8.0, 0.0, 8.0, 15.0]);
        assert!(frame.pieces.contains(&Piece::Fill {
            rect: [8.0, 0.0, 8.0, 15.0],
            ink: Ink::solid(palette().foreground),
        }));
        let Some(Piece::Text { ink, .. }) = frame.pieces.last() else {
            panic!("the cursor's cell is the last run");
        };
        assert_eq!(ink.rgb, palette().background);

        let frame = build(&mut cells_of(&["ab"]).into_iter(), cursor, &view(false));
        assert!(!frame.blinking);
        let outline = frame
            .pieces
            .iter()
            .filter(|piece| matches!(piece, Piece::Fill { .. }))
            .count();
        assert_eq!(outline, 4);
    }

    #[test]
    fn a_blinking_cursor_is_not_drawn_in_its_hidden_half() {
        let cursor = Cursor {
            line: 0,
            column: 0,
            shape: CursorShape::Beam,
            blinking: true,
            wide: false,
        };
        let shown = build(&mut std::iter::empty(), cursor, &view(true));
        assert_eq!(shown.pieces.len(), 1);
        let hidden = View {
            blink_on: false,
            ..view(true)
        };
        assert!(build(&mut std::iter::empty(), cursor, &hidden)
            .pieces
            .is_empty());
    }

    #[test]
    fn composing_text_is_drawn_underlined_at_the_cursor_with_a_bar_after_it() {
        let cursor = Cursor {
            line: 1,
            column: 2,
            shape: CursorShape::Block,
            blinking: true,
            wide: false,
        };
        let composing = View {
            composing: "か",
            ..view(true)
        };
        let frame = build(&mut std::iter::empty(), cursor, &composing);
        assert_eq!(texts(&frame), [("か", [16.0, 15.0], 1)]);
        // The bar follows the two-cell character.
        assert_eq!(
            frame.pieces.last(),
            Some(&Piece::Fill {
                rect: [32.0, 15.0, 1.0, 15.0],
                ink: Ink::solid(palette().foreground),
            })
        );
    }

    #[test]
    fn underlines_and_strikeouts_are_lines_on_the_cell() {
        let mut cells = cells_of(&["ab"]);
        cells[0].style.underline = Underline::Double;
        cells[1].style.underline = Underline::Double;
        cells[1].style.strikeout = true;
        let frame = build(&mut cells.into_iter(), hidden_cursor(), &view(true));
        let lines: Vec<[f32; 4]> = frame
            .pieces
            .iter()
            .filter_map(|piece| match piece {
                Piece::Fill { rect, .. } => Some(*rect),
                Piece::Text { .. } => None,
            })
            .collect();
        assert_eq!(
            lines,
            [
                [0.0, 12.0, 8.0, 1.0],
                [0.0, 14.0, 8.0, 1.0],
                [8.0, 12.0, 8.0, 1.0],
                [8.0, 14.0, 8.0, 1.0],
                [8.0, 6.0, 8.0, 1.0],
            ]
        );
    }
}
