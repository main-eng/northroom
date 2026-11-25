//! Shows a font file (`.ttf`, `.otf`, a collection `.ttc` or `.otc`, a Mac `.dfont`) as a
//! specimen: a page for each face it holds, with the face's name and its letters, figures and a
//! sentence at several sizes, every glyph drawn from the face's own outlines.
//!
//! A character the face has no glyph for is left out; a line longer than the page is cut at its
//! edge. A variable face is drawn at its default instance.

use super::{
    read_bytes, Align, Anchor, Body, Colour, Command, Deck, Figure, Fit, Form, Item, Paragraph,
    ReadError, Result, Run, Shape, Slide, Spacing,
};
use crate::geometry::Rect;
use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::raw::{FileRef, FontRef, TableProvider};
use skrifa::string::StringId;
use skrifa::MetadataProvider;

/// A specimen page's width and margin, in points.
const PAGE_WIDTH: f64 = 720.0;
const MARGIN: f64 = 48.0;
/// The face's name above its specimen, in the app's face.
const NAME_POINTS: f64 = 22.0;
const DETAIL_POINTS: f64 = 12.0;
const DETAIL_COLOUR: Colour = Colour::opaque(0x6b, 0x6b, 0x6b);
/// What is drawn in the face: the alphabet and figures at one size, then a sentence at each
/// size.
const ALPHABET: [&str; 3] = [
    "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    "abcdefghijklmnopqrstuvwxyz",
    "0123456789 .,;:!?&@#%()",
];
const ALPHABET_POINTS: f64 = 32.0;
const SENTENCE: &str = "The quick brown fox jumps over the lazy dog.";
const SENTENCE_POINTS: [f64; 6] = [12.0, 18.0, 24.0, 36.0, 48.0, 72.0];
/// The space between lines, as a share of their size.
const LEADING: f64 = 1.3;
/// The most faces of a collection shown.
const FACES_MAX: usize = 32;

/// Collects a glyph's outline as commands, moving it to its place on the page. The font's y
/// grows upward; the page's grows downward.
struct Pen<'a> {
    commands: &'a mut Vec<Command>,
    x: f64,
    baseline: f64,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> (f64, f64) {
        (self.x + f64::from(x), self.baseline - f64::from(y))
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.commands.push(Command::Move(x, y));
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.commands.push(Command::Line(x, y));
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let ((cx, cy), (x, y)) = (self.at(cx0, cy0), self.at(x, y));
        self.commands.push(Command::Quad(cx, cy, x, y));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let ((ax, ay), (bx, by), (x, y)) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.commands.push(Command::Cubic(ax, ay, bx, by, x, y));
    }

    fn close(&mut self) {
        self.commands.push(Command::Close);
    }
}

/// `text` set in `face` at `size` points with its baseline at `baseline`, as one figure. A line
/// longer than the page ends after its last word that fits, or within its first word when even
/// that is too long.
fn set_line(face: &FontRef, text: &str, size: f64, baseline: f64) -> Figure {
    let charmap = face.charmap();
    let outlines = face.outline_glyphs();
    let metrics = face.glyph_metrics(Size::new(size as f32), LocationRef::default());
    let glyphs: Vec<(char, skrifa::GlyphId, f64)> = text
        .chars()
        .filter_map(|character| {
            let glyph = charmap.map(character)?;
            Some((
                character,
                glyph,
                f64::from(metrics.advance_width(glyph).unwrap_or(0.0)),
            ))
        })
        .collect();
    let shown = fitting(&glyphs, PAGE_WIDTH - 2.0 * MARGIN);
    let mut commands = Vec::new();
    let mut x = MARGIN;
    for (_, glyph, advance) in &glyphs[..shown] {
        if let Some(outline) = outlines.get(*glyph) {
            let mut pen = Pen {
                commands: &mut commands,
                x,
                baseline,
            };
            let settings = DrawSettings::unhinted(Size::new(size as f32), LocationRef::default());
            // A glyph whose outline can't be drawn is left out; the line goes on.
            let _ = outline.draw(settings, &mut pen);
        }
        x += advance;
    }
    Figure {
        commands,
        colour: Colour::BLACK,
    }
}

/// How many of `glyphs` fit `width`: up to the last space before the edge, or as many as fit
/// when there is none.
fn fitting(glyphs: &[(char, skrifa::GlyphId, f64)], width: f64) -> usize {
    let mut used = 0.0;
    let mut last_space = None;
    for (index, (character, _, advance)) in glyphs.iter().enumerate() {
        if used + advance > width {
            return last_space.unwrap_or(index);
        }
        if *character == ' ' {
            last_space = Some(index);
        }
        used += advance;
    }
    glyphs.len()
}

fn label(text: &str, size: f64, bold: bool, colour: Colour, top: f64) -> Item {
    let paragraph = Paragraph {
        align: Align::Start,
        left: 0.0,
        indent: 0.0,
        line: Spacing::Share(1.0),
        before: Spacing::Points(0.0),
        after: Spacing::Points(0.0),
        bullet: None,
        size,
        runs: vec![Run {
            text: text.to_owned(),
            size,
            bold,
            italic: false,
            underline: false,
            colour,
        }],
    };
    Item::Shape(Shape {
        frame: Rect::new(MARGIN, top, PAGE_WIDTH - 2.0 * MARGIN, size * LEADING),
        form: Form::Box,
        fill: None,
        stroke: None,
        text: Some(Body {
            insets: [0.0; 4],
            anchor: Anchor::Top,
            wraps: false,
            scale: 1.0,
            spacing_scale: 1.0,
            paragraphs: vec![paragraph],
        }),
    })
}

/// A name the face gives itself, in English where it has one.
fn name_of(face: &FontRef, id: StringId) -> Option<String> {
    face.localized_strings(id)
        .english_or_first()
        .map(|name| name.to_string())
        .filter(|name| !name.trim().is_empty())
}

/// A specimen page for one face, and how tall it is.
fn specimen(face: &FontRef) -> (Slide, f64) {
    let family = name_of(face, StringId::FULL_NAME)
        .or_else(|| name_of(face, StringId::FAMILY_NAME))
        .unwrap_or_else(|| "Untitled face".to_owned());
    let glyphs = face.maxp().map(|maxp| maxp.num_glyphs()).unwrap_or(0);
    let version = name_of(face, StringId::VERSION_STRING).unwrap_or_default();
    let mut items = vec![label(&family, NAME_POINTS, true, Colour::BLACK, MARGIN)];
    let mut top = MARGIN + NAME_POINTS * LEADING;
    let detail = format!("{glyphs} glyphs  ·  {version}");
    items.push(label(
        detail.trim_end_matches("  ·  "),
        DETAIL_POINTS,
        false,
        DETAIL_COLOUR,
        top,
    ));
    top += DETAIL_POINTS * LEADING * 2.0;
    let lines = ALPHABET
        .iter()
        .map(|text| (*text, ALPHABET_POINTS))
        .chain(SENTENCE_POINTS.iter().map(|size| (SENTENCE, *size)));
    for (text, size) in lines {
        let baseline = top + size;
        items.push(Item::Figure(set_line(face, text, size, baseline)));
        top += size * LEADING;
    }
    let height = top + MARGIN;
    (
        Slide {
            ground: Colour::WHITE,
            items,
        },
        height,
    )
}

/// The font files a Mac font suitcase's data holds: its resources of type `sfnt`.
fn suitcase_fonts(bytes: &[u8]) -> Vec<&[u8]> {
    let be16 = |at: usize| {
        bytes
            .get(at..at + 2)
            .map(|two| usize::from(u16::from_be_bytes([two[0], two[1]])))
    };
    let be32 = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|four| u32::from_be_bytes(four.try_into().expect("four bytes")) as usize)
    };
    let (Some(data), Some(map)) = (be32(0), be32(4)) else {
        return Vec::new();
    };
    let Some(types) = be16(map + 24).map(|offset| map + offset) else {
        return Vec::new();
    };
    let count = be16(types).map_or(0, |count| count + 1);
    let mut fonts = Vec::new();
    for index in 0..count.min(64) {
        let entry = types + 2 + index * 8;
        if bytes.get(entry..entry + 4) != Some(b"sfnt".as_slice()) {
            continue;
        }
        let resources = be16(entry + 4).map_or(0, |count| count + 1);
        let list = types + be16(entry + 6).unwrap_or(0);
        for resource in 0..resources.min(FACES_MAX) {
            let place = be32(list + resource * 12 + 4).map_or(0, |value| value & 0x00ff_ffff);
            let at = data + place;
            let length = be32(at).unwrap_or(0);
            if let Some(font) = bytes.get(at + 4..at + 4 + length) {
                fonts.push(font);
            }
        }
    }
    fonts
}

/// The faces a font file holds: one, a collection's, or a suitcase's.
fn faces(bytes: &[u8]) -> Vec<FontRef<'_>> {
    match FileRef::new(bytes) {
        Ok(FileRef::Font(face)) => vec![face],
        Ok(FileRef::Collection(collection)) => {
            collection.iter().take(FACES_MAX).flatten().collect()
        }
        Err(_) => suitcase_fonts(bytes)
            .into_iter()
            .filter_map(|font| FontRef::new(font).ok())
            .collect(),
    }
}

/// Reads a font file held in memory as specimen pages, one a face.
pub fn parse(bytes: &[u8]) -> Result<Deck> {
    let pages: Vec<(Slide, f64)> = faces(bytes).iter().map(specimen).collect();
    let height = pages.iter().map(|(_, height)| *height).fold(0.0, f64::max);
    if pages.is_empty() {
        return Err(ReadError::NotItsKind);
    }
    Ok(Deck {
        width: PAGE_WIDTH,
        height,
        fit: Fit::Width,
        slides: pages.into_iter().map(|(slide, _)| slide).collect(),
        pictures: Vec::new(),
    })
}

/// Reads the font file at `path`.
pub fn read(path: &std::path::Path) -> Result<Deck> {
    parse(&read_bytes(path)?)
}
