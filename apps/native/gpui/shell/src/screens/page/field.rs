//! The text field for editing a page block. It holds the block's Markdown source (all offsets
//! index into it) and renders it from the core's styled spans (`inline_style`): Markdown marks
//! take no space, text can be bold, italic, struck, code or a link, and each image shows on its
//! own line as the page draws it at rest. Text draws with the same routine as the block at rest
//! (`draw_word_lines` with the page setting), using the same runs that position the caret and
//! selection, so entering edit mode or selecting moves no glyph.
//!
//! The core's page editor makes every decision. This keeps the text, selection and input-method
//! marked text, lays them out and draws them.

use crate::screens::page::marks;
use gpui::{App, Bounds, Hsla, Pixels, Point, Window, point, px};
use kit::controls::Faced;
use kit::controls::Font;
use kit::controls::edit::{from_utf16, range_from_utf16, range_to_utf16, to_utf16};
use kit::core::{c, frame, text};
use kit::nr::*;
use kit::text::{Run, SetLine, Setting, draw_word_lines, set};
use kit::theme::{Face, Token, color, mixed};
use northroom_core::page::editor::Typing;
use std::ffi::CStr;
use std::ops::Range;

/// Where a block's text sits and its type style (`NrPageSlot`), in page points.
#[derive(Clone, Debug, PartialEq)]
pub struct Slot {
    pub block: i64,
    pub frame: Bounds<Pixels>,
    pub font: Font,
    pub line_height: f32,
    pub tracking: f32,
    /// A table cell's row and column; -1 for other blocks.
    pub row: i64,
    pub column: i64,
    /// A board media block's caption: one line of plain text.
    pub caption: bool,
    /// The theme colour behind the text when it differs from the page background.
    pub ground: Option<String>,
}

impl Slot {
    pub fn of(slot: &NrPageSlot) -> Self {
        let ground = text(&slot.ground);
        Self {
            block: slot.block,
            frame: kit::core::rect(&slot.frame),
            font: Font {
                size: slot.font.size,
                weight: slot.font.weight,
                mono: slot.font.mono,
                slanted: slot.font.slanted,
                tabular: slot.font.tabular,
            },
            line_height: slot.line_height as f32,
            tracking: slot.tracking as f32,
            row: slot.row,
            column: slot.column,
            caption: slot.caption,
            ground: (!ground.is_empty()).then_some(ground),
        }
    }

    /// The core's field kind (`NrPageEditor.field`): 1 title, 2 block text, 3 table cell,
    /// 4 caption.
    pub fn field(&self) -> u32 {
        if self.block < 0 {
            1
        } else if self.caption {
            4
        } else if self.row >= 0 {
            3
        } else {
            2
        }
    }

    /// The slot's text face.
    fn face(&self) -> Face {
        if self.font.mono {
            Face::mono(self.font.size as f32)
        } else {
            Face::sans(self.font.size as f32, self.font.weight as f32)
        }
    }

    /// The face whose height is centred in each line: the line's size at weight 400, or the
    /// code face.
    fn line_face(&self) -> Face {
        if self.font.mono {
            self.face()
        } else {
            Face::sans(self.font.size as f32, 400.0)
        }
    }
}

// MARK: Rendering the edited line (`inline_style`)

/// What a displayed run represents in the source text.
#[derive(Clone, Debug, PartialEq)]
enum Stands {
    /// The source text itself, byte for byte.
    Written,
    /// A stand-in: a line end, an image's text, or the empty line after a final image.
    Other,
}

/// One run of a stretch, with the source text it represents.
struct Shown {
    run: Run,
    written: Range<usize>,
    stands: Stands,
}

/// Lines of one height drawn together, or a picture on its own line.
enum Stretch {
    Words {
        shown: Vec<Shown>,
        line_height: f32,
        line_face: Face,
    },
    Picture {
        id: String,
        width: f32,
        height: f32,
        line_height: f32,
        written: Range<usize>,
        /// The line ends after it, drawn as part of its line.
        ends: usize,
    },
}

/// One laid-out line: its top and height in the field, its stretch and, for text, its line
/// within that stretch.
struct Line {
    top: f32,
    height: f32,
    stretch: usize,
    set: Option<usize>,
}

/// The candidate offset closest to `x` among those seen so far.
struct Nearest {
    x: f32,
    /// The distance from `x` and the offset.
    best: Option<(f32, usize)>,
}

impl Nearest {
    fn consider(&mut self, at: f32, index: usize) {
        let distance = (at - self.x).abs();
        if self.best.is_none_or(|(found, _)| distance < found) {
            self.best = Some((distance, index));
        }
    }
}

/// The displayed text, laid out.
pub struct Laid {
    stretches: Vec<Stretch>,
    /// Each text stretch's lines as `kit::text::set` breaks them.
    sets: Vec<Vec<SetLine>>,
    lines: Vec<Line>,
    width: f32,
    /// The images, `![alt](src)` whole (UTF-8).
    pub images: Vec<Range<usize>>,
    key: (String, Range<usize>, u32),
}

/// The stretches the field draws and measures: the source text as displayed.
fn shown_stretches(field: &Field) -> (Vec<Stretch>, Vec<Range<usize>>) {
    let slot = &field.slot;
    let mut shown = StretchBuilder {
        field,
        stretches: Vec::new(),
        last: None,
    };
    if !field.formatted {
        // The field's text in its own style, with the title's letters drawn closer.
        let all = 0..field.text.len();
        let mut words = slot_words(slot, field.text.clone(), all, Stands::Written);
        words.run.tracking = slot.tracking;
        shown.push_words(words, slot.line_height, slot.line_face());
        return (shown.stretches, Vec::new());
    }
    let written = c(&field.text);
    let selection = range_to_utf16(&field.text, &field.selected);
    let style = inline_style(
        &field.text,
        selection.start,
        selection.end,
        slot.font.size,
        slot.font.weight,
        f64::from(slot.frame.size.width),
    );
    let images: Vec<Range<usize>> = style
        .images
        .iter()
        .map(|&(start, end)| range_from_utf16(&field.text, &(start..end)))
        .collect();
    let stand_ins: Vec<StandIn> = style
        .images
        .iter()
        .map(|&(start, _)| StandIn::of(slot, &written, start))
        .collect();
    let mut ends_with_break = false;
    for span in &style.spans {
        let range = range_from_utf16(&field.text, &(span.start..span.end));
        if span.line_break {
            ends_with_break = true;
            shown.push_line_end(range);
            continue;
        }
        if span.hidden && span.stand_in.is_none() {
            continue;
        }
        ends_with_break = false;
        match span.stand_in {
            Some(image) => shown.push_stand_in(&stand_ins[image], range),
            None => shown.push_written(span, range),
        }
    }
    if ends_with_break {
        // The line after a final image, where the caret sits after it, as at rest.
        let end = field.text.len();
        shown.push_words(
            slot_words(slot, "\u{200B}".to_owned(), end..end, Stands::Other),
            slot.line_height,
            slot.line_face(),
        );
    }
    (shown.stretches, images)
}

/// A run in the slot's own style and colour, representing `written`.
fn slot_words(slot: &Slot, text: String, written: Range<usize>, stands: Stands) -> Shown {
    Shown {
        run: Run {
            text,
            face: slot.face(),
            color: color(if slot.caption {
                Token::MutedForeground
            } else {
                Token::Foreground
            }),
            underline: None,
            strike: false,
            code: false,
            space: 0.0,
            tracking: 0.0,
        },
        written,
        stands,
    }
}

/// How an image displays: a picture at the core's size, or text.
enum StandIn {
    Picture {
        id: String,
        width: f32,
        height: f32,
    },
    Words {
        words: String,
        face: Face,
        color: Hsla,
        line_height: f32,
    },
}

impl StandIn {
    /// How the image whose `![` is at UTF-16 offset `start` of `written` displays.
    fn of(slot: &Slot, written: &CStr, start: usize) -> Self {
        let mut kind = NrImageShows::Missing;
        // SAFETY: a NUL-terminated string and a kind to fill.
        let shown = text(&unsafe { nr_inline_image(written.as_ptr(), start, &mut kind) });
        match kind {
            NrImageShows::Missing => StandIn::Words {
                words: non_blank(shown),
                face: Face::sans(13.0, 400.0),
                color: color(Token::MutedForeground),
                line_height: 20.8 + 12.0,
            },
            NrImageShows::Picture => match picture_size(slot, &shown) {
                Some((width, height)) => StandIn::Picture {
                    id: shown,
                    width,
                    height,
                },
                // An unreadable picture also shows as a broken `<img>`.
                None => StandIn::broken(slot, String::new()),
            },
            NrImageShows::Words => StandIn::broken(slot, shown),
        }
    }

    /// An image shown as text in the slot's style, like a broken `<img>`.
    fn broken(slot: &Slot, words: String) -> Self {
        StandIn::Words {
            words: non_blank(words),
            face: slot.face(),
            color: color(Token::Foreground),
            line_height: slot.line_height + 12.0,
        }
    }
}

/// The size the core draws picture `id` at in the slot's width, if readable.
fn picture_size(slot: &Slot, id: &str) -> Option<(f32, f32)> {
    let id = c(id);
    let (mut width, mut height) = (0.0, 0.0);
    // SAFETY: a NUL-terminated id and two sizes to fill.
    let read = unsafe {
        nr_page_picture_size(
            id.as_ptr(),
            f64::from(slot.frame.size.width),
            &mut width,
            &mut height,
        )
    };
    read.then_some((width as f32, height as f32))
}

/// Text that takes a line's height even when empty.
fn non_blank(words: String) -> String {
    if words.is_empty() {
        "\u{00A0}".to_owned()
    } else {
        words
    }
}

/// Builds stretches from the core's spans, in order.
struct StretchBuilder<'a> {
    field: &'a Field,
    stretches: Vec<Stretch>,
    /// The run, line height and face of the last displayed item, used for a line end.
    last: Option<(Run, f32, Face)>,
}

impl StretchBuilder<'_> {
    /// Adds a run to the last text stretch when the line metrics match, or starts a new one.
    fn push_words(&mut self, shown: Shown, line_height: f32, line_face: Face) {
        match self.stretches.last_mut() {
            Some(Stretch::Words {
                shown: runs,
                line_height: height,
                line_face: face,
            }) if *height == line_height && *face == line_face => runs.push(shown),
            _ => self.stretches.push(Stretch::Words {
                shown: vec![shown],
                line_height,
                line_face,
            }),
        }
    }

    /// Adds the line ends in `range`.
    fn push_line_end(&mut self, range: Range<usize>) {
        let count = self.field.text[range.clone()].encode_utf16().count();
        match self.stretches.last_mut() {
            // After a picture, the line end belongs to its line.
            Some(Stretch::Picture { written, ends, .. }) if self.last.is_none() => {
                written.end = range.end;
                *ends += count;
            }
            _ => {
                let (like, height, face) = self.last_shown();
                let run = Run {
                    text: "\n".repeat(count),
                    space: 0.0,
                    ..like
                };
                self.push_words(
                    Shown {
                        run,
                        written: range,
                        stands: Stands::Other,
                    },
                    height,
                    face,
                );
            }
        }
    }

    /// The run, line height and face of the last displayed item, or the slot's own if none.
    fn last_shown(&self) -> (Run, f32, Face) {
        let slot = &self.field.slot;
        match &self.last {
            Some((run, height, face)) => (clone_run(run), *height, *face),
            None => {
                let shown = slot_words(slot, String::new(), 0..0, Stands::Other);
                (shown.run, slot.line_height, slot.line_face())
            }
        }
    }

    /// Adds an image as displayed, representing its source `range`.
    fn push_stand_in(&mut self, stand_in: &StandIn, range: Range<usize>) {
        match stand_in {
            StandIn::Picture { id, width, height } => {
                self.stretches.push(Stretch::Picture {
                    id: id.clone(),
                    width: *width,
                    height: *height,
                    line_height: height + 12.0,
                    written: range,
                    ends: 0,
                });
                self.last = None;
            }
            StandIn::Words {
                words,
                face,
                color,
                line_height,
            } => {
                let run = Run {
                    text: words.clone(),
                    face: *face,
                    color: *color,
                    underline: None,
                    strike: false,
                    code: false,
                    space: 0.0,
                    tracking: 0.0,
                };
                let line_face = Face::sans(face.size, 400.0);
                self.last = Some((clone_run(&run), *line_height, line_face));
                self.push_words(
                    Shown {
                        run,
                        written: range,
                        stands: Stands::Other,
                    },
                    *line_height,
                    line_face,
                );
            }
        }
    }

    /// Adds the source text of `span`, styled as the core says.
    fn push_written(
        &mut self,
        span: &northroom_core::page::inline_edit::Shown,
        range: Range<usize>,
    ) {
        let field = self.field;
        let slot = &field.slot;
        let face = span.face.map_or(slot.face(), |font| font.face());
        let ink = color(Token::Foreground);
        let run = Run {
            text: String::new(),
            face,
            color: if span.muted {
                color(Token::MutedForeground)
            } else {
                ink
            },
            underline: span.underline.then(|| mixed(Token::Foreground, 0.4)),
            strike: span.strike,
            code: span.code,
            space: 0.0,
            tracking: 0.0,
        };
        // Space after each character of the stretch: inline code padding.
        let pieces: Vec<Range<usize>> = match span.kern {
            Some(_) => field.text[range.clone()]
                .char_indices()
                .map(|(at, character)| range.start + at..range.start + at + character.len_utf8())
                .collect(),
            None => vec![range.clone()],
        };
        for piece in pieces {
            self.push_words(
                Shown {
                    run: Run {
                        text: field.text[piece.clone()].to_owned(),
                        ..clone_run(&run)
                    },
                    written: piece.clone(),
                    stands: Stands::Written,
                },
                slot.line_height,
                slot.line_face(),
            );
            if let Some(kern) = span.kern {
                self.push_words(
                    Shown {
                        run: Run {
                            space: kern as f32,
                            ..clone_run(&run)
                        },
                        written: piece.end..piece.end,
                        stands: Stands::Other,
                    },
                    slot.line_height,
                    slot.line_face(),
                );
            }
        }
        self.last = Some((run, slot.line_height, slot.line_face()));
    }
}

fn clone_run(run: &Run) -> Run {
    Run {
        text: run.text.clone(),
        face: run.face,
        color: run.color,
        underline: run.underline,
        strike: run.strike,
        code: run.code,
        space: run.space,
        tracking: run.tracking,
    }
}

impl Laid {
    fn of(field: &Field) -> Self {
        let (stretches, images) = shown_stretches(field);
        let width = f32::from(field.slot.frame.size.width);
        let mut sets = Vec::new();
        let mut lines = Vec::new();
        let mut top = 0.0;
        for (index, stretch) in stretches.iter().enumerate() {
            match stretch {
                Stretch::Words {
                    shown, line_height, ..
                } => {
                    let runs: Vec<Run> = shown.iter().map(|shown| clone_run(&shown.run)).collect();
                    let laid = set(&runs, width, !field.slot.caption);
                    for number in 0..laid.len() {
                        lines.push(Line {
                            top,
                            height: *line_height,
                            stretch: index,
                            set: Some(number),
                        });
                        top += line_height;
                    }
                    sets.push(laid);
                }
                Stretch::Picture { line_height, .. } => {
                    lines.push(Line {
                        top,
                        height: *line_height,
                        stretch: index,
                        set: None,
                    });
                    top += line_height;
                    sets.push(Vec::new());
                }
            }
        }
        if lines.is_empty() {
            // An empty field still has one line.
            lines.push(Line {
                top: 0.0,
                height: field.slot.line_height,
                stretch: usize::MAX,
                set: None,
            });
        }
        Self {
            stretches,
            sets,
            lines,
            width,
            images,
            key: field.key(),
        }
    }

    pub fn height(&self) -> f32 {
        self.lines.last().map_or(0.0, |line| line.top + line.height)
    }

    /// The runs of a text stretch.
    fn shown(&self, stretch: usize) -> &[Shown] {
        match self.stretches.get(stretch) {
            Some(Stretch::Words { shown, .. }) => shown,
            _ => &[],
        }
    }

    /// The x of byte `at` in a run's text on its shaped line, from the line start.
    fn x_in_run(&self, stretch: usize, piece: &kit::text::Piece, at: usize) -> f32 {
        let run = &self.shown(stretch)[piece.run].run;
        if run.space > 0.0 || run.text.is_empty() {
            return piece.x + if at > piece.start { piece.width } else { 0.0 };
        }
        piece.x_of(at)
    }

    /// The line the caret at source offset `index` is on: the first line covering it. An offset
    /// at a line end stays on that line unless the next line starts there.
    fn line_of(&self, index: usize) -> usize {
        for number in 0..self.lines.len() {
            let Some(covers) = self.covers(number) else {
                continue;
            };
            let next = self.covers(number + 1).map(|next| next.start);
            if covers.end > index || (covers.end == index && next.is_none_or(|next| next > index)) {
                return number;
            }
        }
        self.lines.len() - 1
    }

    /// The caret position for source offset `index`: its line and x. Hidden marks sit where the
    /// next visible character does, like null glyphs in an `NSTextView`.
    pub fn place(&self, index: usize) -> (usize, f32) {
        let number = self.line_of(index);
        let line = &self.lines[number];
        let x = match (self.stretches.get(line.stretch), line.set) {
            (Some(Stretch::Picture { written, .. }), _) => {
                if index <= written.start {
                    0.0
                } else {
                    self.width
                }
            }
            (Some(Stretch::Words { shown, .. }), Some(set)) => {
                let pieces = &self.sets[line.stretch][set].pieces;
                let mut x = pieces.last().map_or(0.0, |piece| piece.x + piece.width);
                for piece in pieces {
                    let run = &shown[piece.run];
                    let start = run.written.start;
                    match run.stands {
                        Stands::Written if index <= start + piece.start => {
                            x = piece.x;
                            break;
                        }
                        Stands::Written if index < start + piece.end => {
                            x = self.x_in_run(line.stretch, piece, index - start);
                            break;
                        }
                        Stands::Other if index <= start && run.run.space == 0.0 => {
                            x = piece.x;
                            break;
                        }
                        _ => {}
                    }
                }
                x
            }
            _ => 0.0,
        };
        (number, x)
    }

    /// The source range line `number` covers.
    fn covers(&self, number: usize) -> Option<Range<usize>> {
        let line = self.lines.get(number)?;
        match self.stretches.get(line.stretch)? {
            Stretch::Picture { written, .. } => Some(written.clone()),
            Stretch::Words { shown, .. } => {
                let set_line = &self.sets[line.stretch][line.set?];
                let first = set_line.pieces.first()?;
                let last = set_line.pieces.last()?;
                let start = match shown[first.run].stands {
                    Stands::Written => shown[first.run].written.start + first.start,
                    Stands::Other => shown[first.run].written.start,
                };
                let end = match shown[last.run].stands {
                    Stands::Written => shown[last.run].written.start + last.end,
                    Stands::Other => shown[last.run].written.end,
                };
                Some(start..end)
            }
        }
    }

    /// The source offset on line `number` closest to `x`.
    pub fn index_on_line(&self, number: usize, x: f32) -> usize {
        let line = &self.lines[number];
        let mut nearest = Nearest { x, best: None };
        match (self.stretches.get(line.stretch), line.set) {
            (Some(Stretch::Picture { written, .. }), _) => {
                nearest.consider(0.0, written.start);
                nearest.consider(self.width, written.end);
            }
            (Some(Stretch::Words { shown, .. }), Some(set)) => {
                for piece in &self.sets[line.stretch][set].pieces {
                    let run = &shown[piece.run];
                    match run.stands {
                        Stands::Written => {
                            let words = &run.run.text[piece.start..piece.end];
                            let trimmed = words.trim_end().len();
                            for (offset, _) in words.char_indices() {
                                let at = piece.start + offset;
                                nearest.consider(
                                    self.x_in_run(line.stretch, piece, at),
                                    run.written.start + at,
                                );
                            }
                            // The end of the text, before trailing spaces that hang past the
                            // line.
                            let end = piece.start + trimmed;
                            nearest.consider(
                                self.x_in_run(line.stretch, piece, end),
                                run.written.start + end,
                            );
                        }
                        Stands::Other if run.run.space == 0.0 => {
                            nearest.consider(piece.x, run.written.start);
                        }
                        Stands::Other => {}
                    }
                }
            }
            _ => {}
        }
        nearest.best.map_or_else(
            || {
                self.covers(number)
                    .map_or(self.key.0.len(), |covers| covers.end)
            },
            |(_, index)| index,
        )
    }

    /// The line at field `y`.
    pub fn line_at(&self, y: f32) -> usize {
        self.lines
            .iter()
            .position(|line| y < line.top + line.height)
            .unwrap_or(self.lines.len() - 1)
    }

    /// The source offset under a field point.
    pub fn index_at(&self, at: Point<Pixels>) -> usize {
        self.index_on_line(self.line_at(f32::from(at.y)), f32::from(at.x))
    }

    /// The image whose line is at field `y`, and its placeholder character.
    pub fn image_at(&self, y: f32) -> Option<(Range<usize>, usize)> {
        let covers = self.covers(self.line_at(y))?;
        self.images
            .iter()
            .find(|image| image.start <= covers.start && covers.start < image.end)
            .map(|image| (image.clone(), covers.start))
    }

    /// Whether the source character at `index` takes space. Markdown marks are in the source but
    /// not displayed.
    fn shown_at(&self, index: usize) -> bool {
        self.stretches.iter().any(|stretch| match stretch {
            Stretch::Picture { written, .. } => written.start == index,
            Stretch::Words { shown, .. } => shown
                .iter()
                .any(|shown| shown.written.start <= index && index < shown.written.end),
        })
    }

    /// The top and height of line `number`.
    pub fn line_box(&self, number: usize) -> (f32, f32) {
        let line = &self.lines[number];
        (line.top, line.height)
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// The displayed text, each picture as `[picture]`.
    pub fn drawn(&self) -> String {
        let mut drawn = String::new();
        for stretch in &self.stretches {
            match stretch {
                Stretch::Picture { ends, .. } => {
                    // Line ends after it belong to its line.
                    drawn.push_str("[picture]");
                    drawn.push_str(&"\n".repeat(*ends));
                }
                Stretch::Words { shown, .. } => {
                    for shown in shown {
                        drawn.push_str(&shown.run.text);
                    }
                }
            }
        }
        drawn
    }

    /// The selection rectangles in the field, full height on each line it touches.
    pub fn selection(&self, selected: &Range<usize>) -> Vec<Bounds<Pixels>> {
        if selected.is_empty() {
            return Vec::new();
        }
        (0..self.lines.len())
            .filter_map(|number| self.line_selection(number, selected))
            .collect()
    }

    /// The selection rectangle on text line `number`, if the selection touches it.
    fn line_selection(&self, number: usize, selected: &Range<usize>) -> Option<Bounds<Pixels>> {
        let covers = self.covers(number)?;
        if selected.end <= covers.start || selected.start >= covers.end {
            return None;
        }
        let (top, height) = self.line_box(number);
        // A selected picture gets a tint over it when the text is drawn.
        if matches!(
            self.stretches.get(self.lines[number].stretch),
            Some(Stretch::Picture { .. })
        ) {
            return None;
        }
        let left = if selected.start <= covers.start {
            0.0
        } else {
            self.place(selected.start).1
        };
        let right = if selected.end >= covers.end && number + 1 < self.lines.len() {
            self.width
        } else {
            self.place(selected.end).1
        };
        Some(frame(left, top, (right - left).max(0.0), height))
    }

    /// Underlines the input-method composition `marked`, with the field's top-left at `origin`.
    fn paint_composition(&self, window: &mut Window, marked: &Range<usize>, origin: Point<Pixels>) {
        let (from_line, from) = self.place(marked.start);
        let (to_line, to) = self.place(marked.end);
        for number in from_line..=to_line {
            let (top, height) = self.line_box(number);
            let left = if number == from_line { from } else { 0.0 };
            let right = if number == to_line { to } else { self.width };
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: origin + point(px(left), px(top + height - 4.0)),
                    size: gpui::size(px((right - left).max(1.0)), px(1.0)),
                },
                color(Token::Foreground),
            ));
        }
    }

    /// The field frames of the selected pictures.
    fn pictures_selected(&self, selected: &Range<usize>) -> Vec<Bounds<Pixels>> {
        self.lines
            .iter()
            .filter_map(|line| match self.stretches.get(line.stretch) {
                Some(Stretch::Picture {
                    written,
                    width,
                    height,
                    ..
                }) if selected.start <= written.start
                    && selected.end > written.start
                    && !selected.is_empty() =>
                {
                    Some(frame(0.0, line.top + 6.0, *width, *height))
                }
                _ => None,
            })
            .collect()
    }

    /// Draws the displayed text with the field's top-left at `origin`, as the page draws the
    /// block at rest: each same-height stretch with the page's text routine, each picture on its
    /// own line 6 pt below the line top.
    fn paint_words(&self, window: &mut Window, cx: &mut App, origin: Point<Pixels>) {
        let (x, mut y) = (f32::from(origin.x), f32::from(origin.y));
        for (index, stretch) in self.stretches.iter().enumerate() {
            match stretch {
                Stretch::Picture {
                    id,
                    width,
                    height,
                    line_height,
                    ..
                } => {
                    marks::picture(
                        window,
                        id,
                        2048,
                        frame(x, y + 6.0, *width, *height),
                        color(Token::Muted),
                        6.0,
                    );
                    y += line_height;
                }
                Stretch::Words {
                    shown,
                    line_height,
                    line_face,
                } => {
                    let runs: Vec<Run> = shown.iter().map(|shown| clone_run(&shown.run)).collect();
                    draw_word_lines(
                        window,
                        cx,
                        &runs,
                        x,
                        y,
                        self.width,
                        true,
                        *line_height,
                        *line_face,
                        Setting::Page,
                        None,
                    );
                    y += self.sets[index].len() as f32 * line_height;
                }
            }
        }
    }
}

// MARK: Field

pub struct Field {
    pub slot: Slot,
    /// The Markdown source, as typed.
    pub text: String,
    /// UTF-8 offsets into `text`.
    pub selected: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    /// It holds formatted block text, displayed styled.
    pub formatted: bool,
    /// The x that consecutive ↑ and ↓ presses keep to.
    pub goal: Option<f32>,
    laid: Option<Laid>,
}

impl Field {
    pub fn new(slot: Slot, text: String, caret: usize, formatted: bool) -> Self {
        let caret = from_utf16(&text, caret);
        Self {
            slot,
            text,
            selected: caret..caret,
            reversed: false,
            marked: None,
            formatted,
            goal: None,
            laid: None,
        }
    }

    fn key(&self) -> (String, Range<usize>, u32) {
        (
            self.text.clone(),
            self.selected.clone(),
            f32::from(self.slot.frame.size.width).to_bits(),
        )
    }

    /// The layout for the current text and selection.
    pub fn laid(&mut self) -> &Laid {
        let key = self.key();
        if self.laid.as_ref().is_none_or(|laid| laid.key != key) {
            self.laid = Some(Laid::of(self));
        }
        self.laid.as_ref().expect("laid out just now")
    }

    /// The colours were reloaded, so restyle the text.
    pub fn restyle(&mut self) {
        self.laid = None;
    }

    pub fn caret(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    /// The text and selection as sent to the core (UTF-16 offsets).
    pub fn typing(&self) -> Typing<'_> {
        let chosen = range_to_utf16(&self.text, &self.selected);
        Typing {
            text: &self.text,
            start: chosen.start,
            end: chosen.end,
        }
    }

    pub fn set_text(&mut self, text: String, caret: usize) {
        self.text = text;
        let caret = from_utf16(&self.text, caret).min(self.text.len());
        self.selected = caret..caret;
        self.reversed = false;
        self.marked = None;
    }

    /// Selects `start..end` (UTF-16).
    pub fn select_units(&mut self, start: usize, end: usize) {
        self.selected = range_from_utf16(&self.text, &(start..end.max(start)));
        self.reversed = false;
    }

    /// Moves the selection's caret end to `index`, keeping the other end.
    pub fn select_to(&mut self, index: usize) {
        if self.reversed {
            self.selected.start = index;
        } else {
            self.selected.end = index;
        }
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
    }

    pub fn move_to(&mut self, index: usize) {
        self.selected = index..index;
        self.reversed = false;
    }

    /// The offset one visible character right of `from` (`right`) or left. Surrounding marks are
    /// skipped with it, as the field steps over glyphs.
    pub fn step(&mut self, from: usize, right: bool) -> usize {
        use kit::controls::edit::{next_grapheme, previous_grapheme};
        let length = self.text.len();
        let text = self.text.clone();
        let laid = self.laid();
        let mut at = from;
        loop {
            // The character stepped over, and where the step lands.
            let (passed, landed) = match (right, at) {
                (true, at) if at >= length => return at,
                (true, at) => (at, next_grapheme(&text, at)),
                (false, 0) => return 0,
                (false, at) => {
                    let before = previous_grapheme(&text, at);
                    (before, before)
                }
            };
            at = landed;
            if laid.shown_at(passed) || at == 0 || at >= length {
                return at;
            }
        }
    }

    /// Moves the caret (or the selection's active end) one line up or down, keeping its x, as
    /// `NSTextView.moveUp` does. Up from the first line goes to the start; down from the last
    /// goes to the end.
    pub fn move_vertically(&mut self, up: bool, extend: bool) {
        let caret = self.caret();
        let laid = self.laid();
        let (number, x) = laid.place(caret);
        let count = laid.line_count();
        let goal = *self.goal.get_or_insert(x);
        let laid = self.laid();
        let to = match (up, number) {
            (true, 0) => 0,
            (true, number) => laid.index_on_line(number - 1, goal),
            (false, number) if number + 1 >= count => self.text.len(),
            (false, number) => laid.index_on_line(number + 1, goal),
        };
        if extend {
            self.select_to(to);
        } else {
            self.move_to(to);
        }
    }

    /// The field's frame in the page: as tall as its lines, and at least the slot.
    pub fn frame(&mut self) -> Bounds<Pixels> {
        let slot = self.slot.frame;
        let height = self.laid().height().max(f32::from(slot.size.height));
        Bounds {
            origin: slot.origin,
            size: gpui::size(slot.size.width, px(height)),
        }
    }

    /// The caret's x in the field, and its line's top and height.
    pub fn caret_line(&mut self) -> (f32, f32, f32) {
        let caret = self.caret();
        let laid = self.laid();
        let (number, x) = laid.place(caret);
        let (top, height) = laid.line_box(number);
        (x, top, height)
    }

    /// Draws the field with its top-left at `origin`: background, selection at full line height
    /// (accent at 25%), text, tint over selected pictures, underlined input-method text, and the
    /// caret while visible.
    pub fn paint(
        &mut self,
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        caret_shown: bool,
    ) {
        let field = self.frame();
        let at = |bounds: Bounds<Pixels>| Bounds {
            origin: point(bounds.origin.x + origin.x, bounds.origin.y + origin.y),
            size: bounds.size,
        };
        // Cover the block's at-rest text with its background. The table draws its own cells,
        // and the page draws a caption's placeholder.
        if !self.slot.caption && self.slot.row < 0 {
            let ground = marks::ground(self.slot.ground.as_deref());
            window.paint_quad(gpui::fill(
                at(Bounds {
                    origin: point(field.origin.x, field.origin.y),
                    size: field.size,
                }),
                ground,
            ));
        }
        let selected = self.selected.clone();
        let marked = self.marked.clone();
        let words_at = point(origin.x + field.origin.x, origin.y + field.origin.y);
        let laid = self.laid();
        let tint = mixed(Token::Primary, 0.25);
        for rect in laid.selection(&selected) {
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: words_at + rect.origin,
                    size: rect.size,
                },
                tint,
            ));
        }
        laid.paint_words(window, cx, words_at);
        for picture in laid.pictures_selected(&selected) {
            kit::draw::rounded(
                window,
                Bounds {
                    origin: words_at + picture.origin,
                    size: picture.size,
                },
                tint,
                6.0,
            );
        }
        if let Some(marked) = marked {
            laid.paint_composition(window, &marked, words_at);
        }
        if caret_shown && selected.is_empty() {
            let (x, top, height) = self.caret_line();
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: words_at + point(px(x), px(top)),
                    size: gpui::size(px(1.0), px(height)),
                },
                color(Token::Foreground),
            ));
        }
    }

    /// Where `range` (UTF-8) is drawn relative to the field's top-left, for the input method's
    /// candidate window.
    pub fn bounds_of(&mut self, range: &Range<usize>) -> Bounds<Pixels> {
        let laid = self.laid();
        let (line, from) = laid.place(range.start);
        let (_, to) = laid.place(range.end);
        let (top, height) = laid.line_box(line);
        frame(from, top, (to - from).max(0.0), height)
    }

    /// For a check: the caret's line (top and height, rounded), and each line's top, height,
    /// first source unit and unit count.
    pub fn trace_lines(&mut self) -> (Vec<i64>, Vec<Vec<i64>>) {
        let caret = self.caret();
        let text = self.text.clone();
        let laid = self.laid();
        let (number, _) = laid.place(caret);
        let (top, height) = laid.line_box(number);
        let lines = (0..laid.line_count())
            .map(|number| {
                let (top, height) = laid.line_box(number);
                let covers = laid.covers(number).unwrap_or(text.len()..text.len());
                vec![
                    top.round() as i64,
                    height.round() as i64,
                    to_utf16(&text, covers.start) as i64,
                    (to_utf16(&text, covers.end) - to_utf16(&text, covers.start)) as i64,
                ]
            })
            .collect();
        (vec![top.round() as i64, height.round() as i64], lines)
    }
}
