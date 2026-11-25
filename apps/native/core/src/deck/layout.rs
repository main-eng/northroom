//! Places a deck for its view: the slides one under another, each as large as fits the view,
//! and each slide's shapes, pictures and lines of text where the view draws them.
//!
//! Text is set here, in the app's own face: runs are broken into lines at the spaces that fit
//! their frame, lines sit left, centred or right, and the block sits at the top, middle or
//! bottom of its frame. Text is never shrunk or clipped to its frame: a file's faces are
//! narrower or wider than the app's, so a line may end a little short of or past the frame.

use super::{
    Align, Anchor, Body, Colour, Deck, Fit, Form, Item, Paragraph, Run, Shape, Spacing, Stroke,
};
use crate::board::card::Font;
use crate::dialog::layout::Measure;
use crate::geometry::Rect;

/// The largest a document's page is drawn, as a share of its own size: across a wide view it
/// stops growing, so its text stays at reading size.
const PAGE_ZOOM_MAX: f64 = 1.5;
/// The space around the slides and between two of them.
pub const MARGIN: f64 = 24.0;
pub const GAP: f64 = 16.0;
/// A line's height as a share of its largest text's size, at single spacing.
const LINE_SHARE: f64 = 1.2;
/// An underline's distance below the middle of its line and its thickness, as shares of the
/// text's size.
const UNDERLINE_DROP: f64 = 0.42;
const UNDERLINE_THICKNESS: f64 = 0.06;

/// Where each slide sits in the view's content, and how tall the content is.
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub slides: Vec<Rect>,
    pub height: f64,
}

/// The slides of `deck` for a view `width` × `height`: each as large as fits inside the
/// margins, centred, one under another.
pub fn page(deck: &Deck, width: f64, height: f64) -> Page {
    let room = (
        (width - 2.0 * MARGIN).max(1.0),
        (height - 2.0 * MARGIN).max(1.0),
    );
    let zoom = match deck.fit {
        Fit::Whole => (room.0 / deck.width).min(room.1 / deck.height),
        Fit::Width => (room.0 / deck.width).min(PAGE_ZOOM_MAX),
        Fit::Picture => (room.0 / deck.width).min(room.1 / deck.height).min(1.0),
    };
    let (slide_width, slide_height) = (deck.width * zoom, deck.height * zoom);
    let left = ((width - slide_width) / 2.0).max(0.0);
    let slides: Vec<Rect> = (0..deck.slides.len())
        .map(|index| {
            let top = MARGIN + index as f64 * (slide_height + GAP);
            Rect::new(left, top, slide_width, slide_height)
        })
        .collect();
    let height = slides
        .last()
        .map_or(0.0, |last| last.y + last.height + MARGIN);
    Page { slides, height }
}

/// One line of text in one look, set in a line box `height` tall at `top`. `line_font` is the
/// largest font on its line, whose baseline every run on the line shares.
#[derive(Clone, Debug, PartialEq)]
pub struct Words {
    pub text: String,
    pub x: f64,
    pub top: f64,
    pub height: f64,
    pub font: Font,
    pub line_font: Font,
    pub colour: Colour,
}

/// One thing a slide's view draws, in the view's points.
#[derive(Clone, Debug, PartialEq)]
pub enum Mark {
    Fill {
        frame: Rect,
        form: Form,
        colour: Colour,
    },
    Outline {
        frame: Rect,
        form: Form,
        stroke: Stroke,
    },
    Picture {
        frame: Rect,
        picture: usize,
        crop: [f64; 4],
    },
    Words(Words),
}

/// Where a slide is drawn: its top-left in the view and how much larger than its own points.
#[derive(Clone, Copy)]
struct Stage {
    x: f64,
    y: f64,
    zoom: f64,
}

impl Stage {
    fn place(self, frame: Rect) -> Rect {
        Rect::new(
            self.x + frame.x * self.zoom,
            self.y + frame.y * self.zoom,
            frame.width * self.zoom,
            frame.height * self.zoom,
        )
    }

    fn form(self, form: Form) -> Form {
        match form {
            Form::Rounded(radius) => Form::Rounded(radius * self.zoom),
            Form::Box | Form::Oval | Form::Line { .. } => form,
        }
    }

    fn font(self, font: Font) -> Font {
        Font {
            size: font.size * self.zoom,
            ..font
        }
    }
}

fn font_of(run: &Run, scale: f64) -> Font {
    Font {
        slanted: run.italic,
        ..Font::sans(
            run.size * scale,
            if run.bold { Font::BOLD } else { Font::REGULAR },
        )
    }
}

/// A stretch of one run on a line.
struct Segment<'a> {
    run: &'a Run,
    text: String,
    width: f64,
}

/// A line being filled: its segments, their width, and its largest text size.
#[derive(Default)]
struct Line<'a> {
    segments: Vec<Segment<'a>>,
    width: f64,
    size: f64,
}

impl<'a> Line<'a> {
    /// Adds a word of `run`, joining it to the segment before when that is the same run.
    fn push(&mut self, run: &'a Run, word: &str, width: f64, size: f64) {
        self.width += width;
        self.size = self.size.max(size);
        match self.segments.last_mut() {
            Some(last) if std::ptr::eq(last.run, run) => {
                last.text.push_str(word);
                last.width += width;
            }
            _ => self.segments.push(Segment {
                run,
                text: word.to_owned(),
                width,
            }),
        }
    }

    /// Drops the spaces that end the line, so they don't push centred or right-set text off
    /// its place.
    fn trim(&mut self, scale: f64, measure: &mut Measure) {
        let Some(last) = self.segments.last_mut() else {
            return;
        };
        let trimmed = last.text.trim_end_matches(' ').len();
        if trimmed == last.text.len() {
            return;
        }
        last.text.truncate(trimmed);
        let width = measure(&last.text, font_of(last.run, scale));
        self.width += width - last.width;
        last.width = width;
    }
}

/// Breaks a paragraph's runs into lines. A word goes to the next line when the line it would
/// join has no room left for it; `room` gives each line's width by its number, and is
/// unbounded for text that does not wrap. A line-break run always starts a line.
fn break_lines<'a>(
    paragraph: &'a Paragraph,
    scale: f64,
    room: impl Fn(usize) -> f64,
    measure: &mut Measure,
) -> Vec<Line<'a>> {
    let mut lines = vec![Line::default()];
    for run in &paragraph.runs {
        let (font, size) = (font_of(run, scale), run.size * scale);
        if run.text == "\n" {
            lines.push(Line::default());
            continue;
        }
        for word in run.text.split_inclusive(' ') {
            let width = measure(word, font);
            let fits = width - measure(" ", font) * f64::from(u8::from(word.ends_with(' ')));
            let line = lines.last().expect("a line");
            if !line.segments.is_empty() && line.width + fits > room(lines.len() - 1) {
                lines.push(Line::default());
            }
            lines
                .last_mut()
                .expect("a line")
                .push(run, word, width, size);
        }
    }
    for line in &mut lines {
        line.trim(scale, measure);
    }
    lines
}

/// A distance in a paragraph whose text is `size` points.
fn distance(spacing: Spacing, size: f64) -> f64 {
    match spacing {
        Spacing::Share(share) => share * size * LINE_SHARE,
        Spacing::Points(points) => points,
    }
}

/// A paragraph broken into lines, with what its placing needs.
struct Set<'a> {
    paragraph: &'a Paragraph,
    lines: Vec<Line<'a>>,
    /// Where its first line and its other lines start from the text's left edge.
    first: f64,
    rest: f64,
    /// The gaps above and below it.
    before: f64,
    after: f64,
}

impl Set<'_> {
    /// A line's height: its largest text at the paragraph's spacing, the spacing shrunk as the
    /// file's fit says. An empty line is as tall as the paragraph's own text.
    fn line_height(&self, line: &Line, body: &Body) -> f64 {
        let size = if line.segments.is_empty() {
            self.paragraph.size * body.scale
        } else {
            line.size
        };
        match self.paragraph.line {
            Spacing::Share(share) => share * body.spacing_scale * size * LINE_SHARE,
            Spacing::Points(points) => points,
        }
    }

    fn height(&self, body: &Body) -> f64 {
        let lines: f64 = self
            .lines
            .iter()
            .map(|line| self.line_height(line, body))
            .sum();
        self.before + lines + self.after
    }
}

fn set_paragraph<'a>(
    paragraph: &'a Paragraph,
    body: &Body,
    width: f64,
    measure: &mut Measure,
) -> Set<'a> {
    let rest = paragraph.left;
    // A bullet hangs in the indent, so the first line's text starts with the others.
    let hangs = paragraph.bullet.is_some() && paragraph.indent < 0.0;
    let first = if hangs { rest } else { rest + paragraph.indent };
    let room = |line: usize| {
        if !body.wraps {
            return f64::INFINITY;
        }
        width - if line == 0 { first } else { rest }
    };
    let size = paragraph.size * body.scale;
    Set {
        paragraph,
        lines: break_lines(paragraph, body.scale, room, measure),
        first,
        rest,
        before: distance(paragraph.before, size),
        after: distance(paragraph.after, size),
    }
}

/// The share of a line's spare room that goes before it.
fn lead(align: Align) -> f64 {
    match align {
        Align::Start => 0.0,
        Align::Center => 0.5,
        Align::End => 1.0,
    }
}

/// What text is set in: the stage, the text's own frame on the slide, and the marks made.
struct Setting<'a> {
    stage: Stage,
    inner: Rect,
    body: &'a Body,
    marks: &'a mut Vec<Mark>,
}

impl Setting<'_> {
    /// Adds one stretch of text at `x`, `top` on the slide, and its underline.
    fn add_words(&mut self, run: &Run, text: &str, at: (f64, f64), line: (f64, f64), width: f64) {
        let (height, line_size) = line;
        let font = font_of(run, self.body.scale);
        let place = self.stage.place(Rect::new(at.0, at.1, width, height));
        self.marks.push(Mark::Words(Words {
            text: text.to_owned(),
            x: place.x,
            top: place.y,
            height: place.height,
            font: self.stage.font(font),
            line_font: self.stage.font(Font {
                size: line_size,
                ..font
            }),
            colour: run.colour,
        }));
        if run.underline {
            let under = Rect::new(
                at.0,
                at.1 + height / 2.0 + font.size * UNDERLINE_DROP,
                width,
                font.size * UNDERLINE_THICKNESS,
            );
            self.marks.push(Mark::Fill {
                frame: self.stage.place(under),
                form: Form::Box,
                colour: run.colour,
            });
        }
    }

    /// Adds one line at `top`: its bullet when it is the paragraph's first, then its segments
    /// from where its alignment starts them.
    fn add_line(&mut self, set: &Set, number: usize, top: f64, measure: &mut Measure) {
        let line = &set.lines[number];
        let height = set.line_height(line, self.body);
        let start = if number == 0 { set.first } else { set.rest };
        let spare = self.inner.width - start - line.width;
        let mut x = self.inner.x + start + spare * lead(set.paragraph.align);
        let sizes = (height, line.size);
        let bullet = set.paragraph.bullet.as_ref().filter(|_| number == 0);
        if let (Some(bullet), Some(first)) = (bullet, line.segments.first()) {
            let width = measure(bullet, font_of(first.run, self.body.scale));
            // A hanging bullet sits in the indent; another pushes the text along.
            let hung = x + set.paragraph.indent.min(0.0);
            self.add_words(first.run, bullet, (hung, top), sizes, width);
            if set.paragraph.indent >= 0.0 {
                x += width + measure(" ", font_of(first.run, self.body.scale));
            }
        }
        for segment in &line.segments {
            self.add_words(segment.run, &segment.text, (x, top), sizes, segment.width);
            x += segment.width;
        }
    }

    fn add_paragraphs(&mut self, sets: &[Set], measure: &mut Measure) {
        let height: f64 = sets.iter().map(|set| set.height(self.body)).sum();
        let spare = self.inner.height - height;
        let mut top = self.inner.y
            + match self.body.anchor {
                Anchor::Top => 0.0,
                Anchor::Middle => spare / 2.0,
                Anchor::Bottom => spare,
            };
        for set in sets {
            top += set.before;
            for number in 0..set.lines.len() {
                self.add_line(set, number, top, measure);
                top += set.line_height(&set.lines[number], self.body);
            }
            top += set.after;
        }
    }
}

/// How tall `body`'s text is when set `width` wide, its insets included: what a document is cut
/// into pages by (`flow`).
pub(crate) fn body_height(body: &Body, width: f64, measure: &mut Measure) -> f64 {
    let [left, top, right, bottom] = body.insets;
    let inner = (width - left - right).max(0.0);
    let text: f64 = body
        .paragraphs
        .iter()
        .map(|paragraph| set_paragraph(paragraph, body, inner, measure).height(body))
        .sum();
    top + text + bottom
}

/// Sets a shape's text inside `frame` on the slide.
fn set_body(body: &Body, frame: Rect, stage: Stage, measure: &mut Measure, marks: &mut Vec<Mark>) {
    let [left, top, right, bottom] = body.insets;
    let inner = Rect::new(
        frame.x + left,
        frame.y + top,
        (frame.width - left - right).max(0.0),
        (frame.height - top - bottom).max(0.0),
    );
    let sets: Vec<Set> = body
        .paragraphs
        .iter()
        .map(|paragraph| set_paragraph(paragraph, body, inner.width, measure))
        .collect();
    let mut setting = Setting {
        stage,
        inner,
        body,
        marks,
    };
    setting.add_paragraphs(&sets, measure);
}

fn add_shape(shape: &Shape, stage: Stage, measure: &mut Measure, marks: &mut Vec<Mark>) {
    let (frame, form) = (stage.place(shape.frame), stage.form(shape.form));
    if let Some(colour) = shape.fill {
        marks.push(Mark::Fill {
            frame,
            form,
            colour,
        });
    }
    if let Some(stroke) = shape.stroke {
        let stroke = Stroke {
            width: stroke.width * stage.zoom,
            ..stroke
        };
        marks.push(Mark::Outline {
            frame,
            form,
            stroke,
        });
    }
    if let Some(body) = &shape.text {
        set_body(body, shape.frame, stage, measure, marks);
    }
}

/// What the view draws for slide `slide` of `deck` at `frame` (`page`), back to front: its
/// background, then everything on it. Empty for a slide the deck does not have.
pub fn marks(deck: &Deck, slide: usize, frame: Rect, measure: &mut Measure) -> Vec<Mark> {
    let Some(slide) = deck.slides.get(slide) else {
        return Vec::new();
    };
    let stage = Stage {
        x: frame.x,
        y: frame.y,
        zoom: frame.width / deck.width,
    };
    let mut marks = vec![Mark::Fill {
        frame,
        form: Form::Box,
        colour: slide.ground,
    }];
    for item in &slide.items {
        match item {
            Item::Shape(shape) => add_shape(shape, stage, measure, &mut marks),
            Item::Picture(placed) => marks.push(Mark::Picture {
                frame: stage.place(placed.frame),
                picture: placed.picture,
                crop: placed.crop,
            }),
        }
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::Slide;

    /// Every character ten points wide at size 20, and in proportion at other sizes.
    fn measure(text: &str, font: Font) -> f64 {
        text.chars().count() as f64 * font.size / 2.0
    }

    fn run(text: &str) -> Run {
        Run {
            text: text.to_owned(),
            size: 20.0,
            bold: false,
            italic: false,
            underline: false,
            colour: Colour::BLACK,
        }
    }

    fn paragraph(runs: Vec<Run>) -> Paragraph {
        Paragraph {
            align: Align::Start,
            left: 0.0,
            indent: 0.0,
            line: Spacing::Share(1.0),
            before: Spacing::Points(0.0),
            after: Spacing::Points(0.0),
            bullet: None,
            size: 20.0,
            runs,
        }
    }

    fn deck_of(body: Body, frame: Rect) -> Deck {
        Deck {
            width: 400.0,
            height: 300.0,
            slides: vec![Slide {
                ground: Colour::WHITE,
                items: vec![Item::Shape(Shape {
                    frame,
                    form: Form::Box,
                    fill: None,
                    stroke: None,
                    text: Some(body),
                })],
            }],
            fit: Fit::Whole,
            pictures: Vec::new(),
        }
    }

    fn body(paragraphs: Vec<Paragraph>) -> Body {
        Body {
            insets: [0.0; 4],
            anchor: Anchor::Top,
            wraps: true,
            scale: 1.0,
            spacing_scale: 1.0,
            paragraphs,
        }
    }

    /// The words set for a deck drawn at its own size: text, x, top.
    fn set(deck: &Deck) -> Vec<(String, f64, f64)> {
        let frame = Rect::new(0.0, 0.0, deck.width, deck.height);
        marks(deck, 0, frame, &mut measure)
            .into_iter()
            .filter_map(|mark| match mark {
                Mark::Words(words) => Some((words.text, words.x, words.top)),
                Mark::Fill { .. } | Mark::Outline { .. } | Mark::Picture { .. } => None,
            })
            .collect()
    }

    #[test]
    fn slides_fit_the_view_and_stack() {
        let mut deck = deck_of(body(vec![]), Rect::new(0.0, 0.0, 1.0, 1.0));
        deck.slides.push(deck.slides[0].clone());
        // Wide view: the height decides. 300 high in 600 of room is twice the size.
        let wide = page(&deck, 2000.0, 648.0);
        assert_eq!(wide.slides[0], Rect::new(600.0, 24.0, 800.0, 600.0));
        assert_eq!(wide.slides[1].y, 24.0 + 600.0 + 16.0);
        assert_eq!(wide.height, 24.0 + 600.0 + 16.0 + 600.0 + 24.0);
        // Narrow view: the width decides.
        let narrow = page(&deck, 248.0, 900.0);
        assert_eq!(narrow.slides[0], Rect::new(24.0, 24.0, 200.0, 150.0));
    }

    #[test]
    fn words_wrap_at_the_frame_and_lines_take_their_alignment() {
        // "one two three" at ten points a character in a frame 80 wide.
        let mut centred = paragraph(vec![run("one two "), run("three")]);
        centred.align = Align::Center;
        let deck = deck_of(body(vec![centred]), Rect::new(100.0, 50.0, 80.0, 200.0));
        assert_eq!(
            set(&deck),
            [
                ("one two".to_owned(), 105.0, 50.0),
                ("three".to_owned(), 115.0, 74.0)
            ]
        );
        // Text that does not wrap stays on one line, each run its own stretch.
        let mut unwrapped = body(vec![paragraph(vec![run("one two "), run("three")])]);
        unwrapped.wraps = false;
        let deck = deck_of(unwrapped, Rect::new(100.0, 50.0, 80.0, 200.0));
        assert_eq!(
            set(&deck),
            [
                ("one two ".to_owned(), 100.0, 50.0),
                ("three".to_owned(), 180.0, 50.0)
            ]
        );
    }

    #[test]
    fn a_block_sits_where_its_anchor_puts_it_inside_its_insets() {
        let mut anchored = body(vec![paragraph(vec![run("a")]), paragraph(vec![run("b")])]);
        anchored.insets = [10.0, 4.0, 10.0, 6.0];
        anchored.anchor = Anchor::Bottom;
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        // Two lines of 24 end at the bottom inset.
        assert_eq!(
            set(&deck_of(anchored.clone(), frame)),
            [("a".to_owned(), 10.0, 46.0), ("b".to_owned(), 10.0, 70.0)]
        );
        anchored.anchor = Anchor::Middle;
        assert_eq!(set(&deck_of(anchored, frame))[0].2, 25.0);
    }

    #[test]
    fn a_bullet_hangs_in_the_indent_and_a_break_starts_a_line() {
        let mut listed = paragraph(vec![run("first"), run("\n"), run("second")]);
        listed.left = 30.0;
        listed.indent = -20.0;
        listed.bullet = Some("•".to_owned());
        listed.before = Spacing::Share(0.5);
        let deck = deck_of(body(vec![listed]), Rect::new(0.0, 0.0, 300.0, 200.0));
        assert_eq!(
            set(&deck),
            [
                ("•".to_owned(), 10.0, 12.0),
                ("first".to_owned(), 30.0, 12.0),
                ("second".to_owned(), 30.0, 36.0)
            ]
        );
    }

    #[test]
    fn everything_scales_with_the_slide() {
        let mut filled = deck_of(
            body(vec![paragraph(vec![run("hi")])]),
            Rect::new(10.0, 10.0, 100.0, 40.0),
        );
        let Item::Shape(shape) = &mut filled.slides[0].items[0] else {
            unreachable!()
        };
        shape.fill = Some(Colour::BLACK);
        shape.form = Form::Rounded(4.0);
        let drawn = marks(
            &filled,
            0,
            Rect::new(50.0, 20.0, 800.0, 600.0),
            &mut measure,
        );
        assert_eq!(
            drawn[1],
            Mark::Fill {
                frame: Rect::new(70.0, 40.0, 200.0, 80.0),
                form: Form::Rounded(8.0),
                colour: Colour::BLACK
            }
        );
        let Mark::Words(words) = &drawn[2] else {
            panic!("the text follows the fill");
        };
        assert_eq!((words.x, words.top, words.height), (70.0, 40.0, 48.0));
        assert_eq!((words.font.size, words.line_font.size), (40.0, 40.0));
        assert!(marks(&filled, 3, Rect::new(0.0, 0.0, 1.0, 1.0), &mut measure).is_empty());
    }
}
