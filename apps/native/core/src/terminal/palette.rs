//! The terminal's colours. Background and text come from the app theme, so the terminal reads
//! as part of the window; the sixteen named colours suit that background.

use super::cells::{rgb, Colour, Rgb};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Rgb,
    pub foreground: Rgb,
    /// Behind selected text: the app's accent surface.
    pub selection: Rgb,
    ansi: &'static [Rgb; 16],
}

/// A colour a program asks the terminal for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Queried {
    /// One of the 256 xterm colours.
    Indexed(u8),
    Foreground,
    Background,
}

/// xterm's sixteen colours, designed for a dark background.
const DARK_GROUND_ANSI: [Rgb; 16] = [
    rgb(0x2e, 0x34, 0x36),
    rgb(0xcc, 0x00, 0x00),
    rgb(0x4e, 0x9a, 0x06),
    rgb(0xc4, 0xa0, 0x00),
    rgb(0x34, 0x65, 0xa4),
    rgb(0x75, 0x50, 0x7b),
    rgb(0x06, 0x98, 0x9a),
    rgb(0xd3, 0xd7, 0xcf),
    rgb(0x55, 0x57, 0x53),
    rgb(0xef, 0x29, 0x29),
    rgb(0x8a, 0xe2, 0x34),
    rgb(0xfc, 0xe9, 0x4f),
    rgb(0x72, 0x9f, 0xcf),
    rgb(0xad, 0x7f, 0xa8),
    rgb(0x34, 0xe2, 0xe2),
    rgb(0xee, 0xee, 0xec),
];

/// For a light background, where xterm's yellows and whites would vanish.
const LIGHT_GROUND_ANSI: [Rgb; 16] = [
    rgb(0x24, 0x29, 0x2f),
    rgb(0xcf, 0x22, 0x2e),
    rgb(0x11, 0x63, 0x29),
    rgb(0x7d, 0x4e, 0x00),
    rgb(0x09, 0x69, 0xda),
    rgb(0x82, 0x50, 0xdf),
    rgb(0x1b, 0x7c, 0x83),
    rgb(0x6e, 0x77, 0x81),
    rgb(0x57, 0x60, 0x6a),
    rgb(0xa4, 0x0e, 0x26),
    rgb(0x1a, 0x7f, 0x37),
    rgb(0x63, 0x3c, 0x01),
    rgb(0x21, 0x8b, 0xff),
    rgb(0xa4, 0x75, 0xf9),
    rgb(0x31, 0x92, 0xaa),
    rgb(0x8c, 0x95, 0x9f),
];

/// A background at least this bright (0–1, weighted towards green like the eye) needs dark
/// text.
const LIGHT_GROUND_MIN_BRIGHTNESS: f32 = 0.5;

impl Palette {
    pub fn on(background: [u8; 3], foreground: [u8; 3], selection: [u8; 3]) -> Self {
        let brightness = (0.2126 * f32::from(background[0])
            + 0.7152 * f32::from(background[1])
            + 0.0722 * f32::from(background[2]))
            / 255.0;
        Self {
            background: rgb(background[0], background[1], background[2]),
            foreground: rgb(foreground[0], foreground[1], foreground[2]),
            selection: rgb(selection[0], selection[1], selection[2]),
            ansi: if brightness >= LIGHT_GROUND_MIN_BRIGHTNESS {
                &LIGHT_GROUND_ANSI
            } else {
                &DARK_GROUND_ANSI
            },
        }
    }

    /// The xterm 256-colour table.
    fn indexed(self, index: u8) -> Rgb {
        match index {
            0..=15 => self.ansi[usize::from(index)],
            16..=231 => {
                let cube = index - 16;
                let level = |step: u8| if step == 0 { 0 } else { 55 + step * 40 };
                rgb(level(cube / 36), level(cube / 6 % 6), level(cube % 6))
            }
            _ => {
                let grey = 8 + (index - 232) * 10;
                rgb(grey, grey, grey)
            }
        }
    }

    /// The colour a program asked for.
    pub fn queried(self, queried: Queried) -> Rgb {
        match queried {
            Queried::Indexed(index) => self.indexed(index),
            Queried::Foreground => self.foreground,
            Queried::Background => self.background,
        }
    }

    /// A cell colour as RGB, or `None` for the terminal's own background. Bold text in one of
    /// the eight plain colours is drawn in its bright one.
    pub fn resolve(self, colour: Colour, bold: bool) -> Option<Rgb> {
        match colour {
            Colour::Background => None,
            Colour::Foreground => Some(self.foreground),
            Colour::Indexed(index) => Some(self.indexed(bright_when_bold(index, bold))),
            Colour::Rgb(rgb) => Some(rgb),
        }
    }
}

/// One of the eight plain colours, drawn in its bright one when the text is bold.
fn bright_when_bold(index: u8, bold: bool) -> u8 {
    if bold && index < 8 {
        index + 8
    } else {
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_resolve_for_drawing_and_for_a_program_that_asks() {
        let palette = Palette::on([0x0b, 0x11, 0x0d], [0xdf, 0xec, 0xe1], [0x20, 0x30, 0x24]);
        assert_eq!(palette.resolve(Colour::Background, false), None);
        assert_eq!(
            palette.resolve(Colour::Indexed(1), false),
            Some(rgb(0xcc, 0x00, 0x00))
        );
        // Bold text in a plain colour is drawn in its bright one.
        assert_eq!(
            palette.resolve(Colour::Indexed(1), true),
            Some(rgb(0xef, 0x29, 0x29))
        );
        assert_eq!(
            palette.resolve(Colour::Indexed(196), true),
            Some(rgb(255, 0, 0))
        );
        assert_eq!(
            palette.resolve(Colour::Rgb(rgb(1, 2, 3)), true),
            Some(rgb(1, 2, 3))
        );
        assert_eq!(palette.queried(Queried::Background), rgb(0x0b, 0x11, 0x0d));
        assert_eq!(palette.queried(Queried::Foreground), rgb(0xdf, 0xec, 0xe1));
        assert_eq!(palette.queried(Queried::Indexed(232)), rgb(8, 8, 8));
    }

    #[test]
    fn a_light_ground_gets_darker_named_colours() {
        let light = Palette::on([0xf4, 0xf4, 0xf2], [0x20, 0x20, 0x20], [0xd0, 0xd8, 0xe0]);
        assert_eq!(
            light.resolve(Colour::Indexed(3), false),
            Some(rgb(0x7d, 0x4e, 0x00))
        );
    }
}
