//! A file's pages as a phone draws them. Every kind the app shows as pages (a PDF, a document,
//! slides, a font's specimen: `deck::Kind`) is the same few marks a page (`deck::layout::Mark`),
//! so a device draws them all with one drawer and reads none of the formats.

use crate::deck::layout::{Mark, Words};
use crate::deck::{Colour, Command, Deck, Form, Stroke};
use crate::geometry::Rect;
use serde::Serialize;
use std::fmt::Write;

/// The narrowest and widest a device may ask a page to be laid out, in points.
const WIDTH_LEAST: f64 = 100.0;
const WIDTH_MOST: f64 = 4000.0;

fn is_false(flag: &bool) -> bool {
    !flag
}

/// A file's pages: how many, and the size every one of them is, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pages {
    pub width: f64,
    pub height: f64,
    pub count: usize,
}

impl Pages {
    pub fn of(deck: &Deck) -> Self {
        Self {
            width: deck.width,
            height: deck.height,
            count: deck.slides.len(),
        }
    }
}

/// One thing a page's view draws, back to front, in points from the page's top left. A frame
/// is its left, top, width and height; a colour is `#rrggbbaa`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "mark", rename_all = "kebab-case")]
pub enum Drawn {
    Fill {
        frame: [f64; 4],
        /// `box`, `rounded`, `oval`, `line` (top left to bottom right) or `rising-line`.
        form: &'static str,
        /// A rounded box's corner radius.
        #[serde(skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
        colour: String,
    },
    Outline {
        frame: [f64; 4],
        form: &'static str,
        #[serde(skip_serializing_if = "Option::is_none")]
        radius: Option<f64>,
        colour: String,
        /// The line's thickness.
        width: f64,
    },
    /// The file's picture numbered `picture`, read with `read`. `crop` is the share cut from
    /// its left, top, right and bottom before it fills the frame.
    Picture {
        frame: [f64; 4],
        picture: usize,
        crop: [f64; 4],
    },
    /// One line of text in one look, in a line box `height` tall at `top`. Its baseline is that
    /// of text `line_size` large, which every run on the line shares.
    Words {
        text: String,
        x: f64,
        top: f64,
        height: f64,
        size: f64,
        /// The weight as CSS numbers it: 400 is regular, 700 bold.
        weight: f64,
        #[serde(skip_serializing_if = "is_false")]
        mono: bool,
        #[serde(skip_serializing_if = "is_false")]
        slanted: bool,
        line_size: f64,
        colour: String,
    },
    /// A filled path, written as SVG writes one (`M`, `L`, `Q`, `C`, `Z`), filled by the
    /// nonzero rule.
    Figure { path: String, colour: String },
}

/// A length to a hundredth of a point, which is finer than a screen shows and shorter to send.
fn rounded(length: f64) -> f64 {
    (length * 100.0).round() / 100.0
}

fn frame_of(frame: Rect) -> [f64; 4] {
    [frame.x, frame.y, frame.width, frame.height].map(rounded)
}

fn colour_of(colour: Colour) -> String {
    let alpha = (colour.alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}{alpha:02x}",
        colour.red, colour.green, colour.blue
    )
}

/// A form's word and, for a rounded box, its radius.
fn form_of(form: Form) -> (&'static str, Option<f64>) {
    match form {
        Form::Box => ("box", None),
        Form::Rounded(radius) => ("rounded", Some(rounded(radius))),
        Form::Oval => ("oval", None),
        Form::Line { rising: false } => ("line", None),
        Form::Line { rising: true } => ("rising-line", None),
    }
}

/// `commands` as an SVG path.
fn path_of(commands: &[Command]) -> String {
    let mut path = String::new();
    for command in commands {
        // Writing to a `String` cannot fail.
        let _ = match *command {
            Command::Move(x, y) => write!(path, "M{} {}", rounded(x), rounded(y)),
            Command::Line(x, y) => write!(path, "L{} {}", rounded(x), rounded(y)),
            Command::Quad(cx, cy, x, y) => write!(
                path,
                "Q{} {} {} {}",
                rounded(cx),
                rounded(cy),
                rounded(x),
                rounded(y)
            ),
            Command::Cubic(ax, ay, bx, by, x, y) => write!(
                path,
                "C{} {} {} {} {} {}",
                rounded(ax),
                rounded(ay),
                rounded(bx),
                rounded(by),
                rounded(x),
                rounded(y)
            ),
            Command::Close => write!(path, "Z"),
        };
    }
    path
}

fn words_of(words: Words) -> Drawn {
    Drawn::Words {
        text: words.text,
        x: rounded(words.x),
        top: rounded(words.top),
        height: rounded(words.height),
        size: rounded(words.font.size),
        weight: words.font.weight,
        mono: words.font.mono,
        slanted: words.font.slanted,
        line_size: rounded(words.line_font.size),
        colour: colour_of(words.colour),
    }
}

fn outline_of(frame: Rect, form: Form, stroke: Stroke) -> Drawn {
    let (form, radius) = form_of(form);
    Drawn::Outline {
        frame: frame_of(frame),
        form,
        radius,
        colour: colour_of(stroke.colour),
        width: rounded(stroke.width),
    }
}

fn drawn_of(mark: Mark) -> Drawn {
    match mark {
        Mark::Fill {
            frame,
            form,
            colour,
        } => {
            let (form, radius) = form_of(form);
            Drawn::Fill {
                frame: frame_of(frame),
                form,
                radius,
                colour: colour_of(colour),
            }
        }
        Mark::Outline {
            frame,
            form,
            stroke,
        } => outline_of(frame, form, stroke),
        Mark::Picture {
            frame,
            picture,
            crop,
        } => Drawn::Picture {
            frame: frame_of(frame),
            picture,
            crop: crop.map(rounded),
        },
        Mark::Words(words) => words_of(words),
        Mark::Figure { commands, colour } => Drawn::Figure {
            path: path_of(&commands),
            colour: colour_of(colour),
        },
    }
}

/// A device's page width, held to widths a page can be laid out at.
pub fn normalize_width(width: f64) -> f64 {
    if width.is_finite() {
        width.clamp(WIDTH_LEAST, WIDTH_MOST)
    } else {
        WIDTH_LEAST
    }
}

/// What a device draws for page `page` of `deck` laid out `width` points wide, back to front.
/// Empty for a page the deck does not have.
pub fn marks(deck: &Deck, page: usize, width: f64) -> Vec<Drawn> {
    let height = width * deck.height / deck.width.max(1.0);
    let frame = Rect::new(0.0, 0.0, width, height);
    crate::deck::layout::marks(deck, page, frame, &mut crate::text::typeset::width)
        .into_iter()
        .map(drawn_of)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::{Fit, Item, Shape, Slide};

    fn colour(red: u8, green: u8, blue: u8) -> Colour {
        Colour {
            red,
            green,
            blue,
            alpha: 1.0,
        }
    }

    fn deck(items: Vec<Item>) -> Deck {
        Deck {
            width: 200.0,
            height: 100.0,
            fit: Fit::Whole,
            slides: vec![Slide {
                ground: colour(255, 255, 255),
                items,
            }],
            pictures: Vec::new(),
        }
    }

    #[test]
    fn a_page_is_its_ground_then_what_is_on_it_scaled_to_the_asked_width() {
        let shape = Item::Shape(Shape {
            frame: Rect::new(10.0, 20.0, 50.0, 30.0),
            form: Form::Rounded(4.0),
            fill: Some(colour(255, 0, 0)),
            stroke: None,
            text: None,
        });
        let deck = deck(vec![shape]);
        assert_eq!(
            Pages::of(&deck),
            Pages {
                width: 200.0,
                height: 100.0,
                count: 1
            }
        );
        let drawn = marks(&deck, 0, 400.0);
        assert_eq!(
            serde_json::to_string(&drawn).unwrap(),
            concat!(
                r##"[{"mark":"fill","frame":[0.0,0.0,400.0,200.0],"form":"box","colour":"#ffffffff"},"##,
                r##"{"mark":"fill","frame":[20.0,40.0,100.0,60.0],"form":"rounded","radius":8.0,"colour":"#ff0000ff"}]"##
            )
        );
        assert_eq!(marks(&deck, 1, 400.0), []);
    }

    #[test]
    fn a_figure_is_written_as_a_path() {
        let commands = [
            Command::Move(0.0, 0.0),
            Command::Line(10.0, 0.0),
            Command::Quad(10.0, 5.0, 5.0, 5.0),
            Command::Cubic(1.0, 2.0, 3.0, 4.0, 0.126, 0.0),
            Command::Close,
        ];
        assert_eq!(path_of(&commands), "M0 0L10 0Q10 5 5 5C1 2 3 4 0.13 0Z");
    }

    #[test]
    fn a_width_no_page_can_be_is_held_to_one_that_can() {
        assert_eq!(normalize_width(390.0), 390.0);
        assert_eq!(normalize_width(1.0), WIDTH_LEAST);
        assert_eq!(normalize_width(f64::NAN), WIDTH_LEAST);
        assert_eq!(normalize_width(1e9), WIDTH_MOST);
    }
}
