//! A presentation read for display (ADR-0015): its slides as shapes, text and pictures in
//! points from each slide's top-left. `pptx` reads a PowerPoint file into these types, and
//! `layout` places them for a view. Nothing here is PowerPoint's: another format's reader makes
//! the same deck.
//!
//! A deck reads right, not exactly: text is set in the app's own face whatever face the file
//! names, shapes other than boxes, ovals and straight lines draw as boxes when they hold text
//! and are left out when they hold none, and rotation, shadows, gradients (drawn as their
//! first colour), charts, animations and transitions are not drawn.

pub mod ffi;
pub mod layout;
pub mod pptx;

use crate::geometry::Rect;

/// A colour as a file gives it, with its opacity from 0 to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colour {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: f64,
}

impl Colour {
    pub const BLACK: Colour = Colour::opaque(0, 0, 0);
    pub const WHITE: Colour = Colour::opaque(255, 255, 255);

    pub const fn opaque(red: u8, green: u8, blue: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    /// The colour written as six hexadecimal digits, with or without `#`.
    pub fn hex(digits: &str) -> Option<Self> {
        let digits = digits.strip_prefix('#').unwrap_or(digits);
        if digits.len() != 6 || !digits.is_ascii() {
            return None;
        }
        let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
        Some(Self::opaque(channel(0)?, channel(2)?, channel(4)?))
    }

    /// This colour moved `share` of the way to `other`, keeping its own opacity.
    pub fn toward(self, other: Colour, share: f64) -> Self {
        let share = share.clamp(0.0, 1.0);
        let mix = |from: u8, to: u8| {
            (f64::from(from) + (f64::from(to) - f64::from(from)) * share).round() as u8
        };
        Self {
            red: mix(self.red, other.red),
            green: mix(self.green, other.green),
            blue: mix(self.blue, other.blue),
            alpha: self.alpha,
        }
    }
}

/// A presentation: the size every slide has, its slides in order, and the pictures they show.
#[derive(Clone, Debug, PartialEq)]
pub struct Deck {
    pub width: f64,
    pub height: f64,
    pub slides: Vec<Slide>,
    /// Each picture's name in the file, which ends in its format's extension. Its bytes stay
    /// in the file until a view draws it (`pptx::read_picture`).
    pub pictures: Vec<String>,
}

/// One slide: its background and what is on it, back to front.
#[derive(Clone, Debug, PartialEq)]
pub struct Slide {
    pub ground: Colour,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Shape(Shape),
    Picture(Placed),
}

/// A picture on a slide: which of the deck's pictures, where, and how much of each edge is cut
/// off before it is stretched to the frame (left, top, right, bottom, as shares of its size).
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub frame: Rect,
    pub picture: usize,
    pub crop: [f64; 4],
}

/// A shape's outline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Form {
    Box,
    /// A box with corners rounded by this radius, in points.
    Rounded(f64),
    Oval,
    /// A straight line between two corners of the frame: top-left to bottom-right, or
    /// bottom-left to top-right when `rising`.
    Line {
        rising: bool,
    },
}

/// A line's colour and thickness in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stroke {
    pub colour: Colour,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub frame: Rect,
    pub form: Form,
    pub fill: Option<Colour>,
    pub stroke: Option<Stroke>,
    pub text: Option<Body>,
}

/// Where text sits in its shape, top to bottom.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Top,
    Middle,
    Bottom,
}

/// Where a paragraph's lines sit, side to side.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
}

/// A distance between lines or paragraphs: a share of the line's own height, or points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spacing {
    Share(f64),
    Points(f64),
}

/// A shape's text: the space kept clear inside the frame (left, top, right, bottom), where the
/// text sits, whether lines wrap at the frame, and the share its sizes and line spacing were
/// shrunk to so it fits.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub insets: [f64; 4],
    pub anchor: Anchor,
    pub wraps: bool,
    pub scale: f64,
    pub spacing_scale: f64,
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Paragraph {
    pub align: Align,
    /// How far the paragraph's lines start from the text's left edge.
    pub left: f64,
    /// How far the first line starts from `left`; negative for a hanging bullet.
    pub indent: f64,
    pub line: Spacing,
    pub before: Spacing,
    pub after: Spacing,
    /// The mark before the first line, drawn like the first run.
    pub bullet: Option<String>,
    /// The size an empty paragraph's line has.
    pub size: f64,
    pub runs: Vec<Run>,
}

/// A stretch of text in one look. A line break is a run whose text is `"\n"`.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub text: String,
    pub size: f64,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub colour: Colour,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_reads_from_hex_and_mixes_toward_another() {
        assert_eq!(Colour::hex("1F6F4A"), Some(Colour::opaque(31, 111, 74)));
        assert_eq!(Colour::hex("#ffffff"), Some(Colour::WHITE));
        assert_eq!(Colour::hex("fff"), None);
        assert_eq!(Colour::hex("zzzzzz"), None);
        assert_eq!(
            Colour::BLACK.toward(Colour::WHITE, 0.5),
            Colour::opaque(128, 128, 128)
        );
    }
}
