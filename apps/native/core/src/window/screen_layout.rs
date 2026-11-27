//! Layout of the first screen, the drag chip and the window's three buttons. Frames are in points
//! from the top-left of the view each is drawn in, and every line of text carries its font and
//! colour (`dialog::layout::Line`). A shell draws backgrounds and glyphs at these frames; text is
//! measured by the core's typesetter (`typeset`).

use crate::board::card::Font;
use crate::dialog::layout::{button, Line, Measure};
use crate::geometry::Rect;
use crate::strings::{self, key};
use crate::theme::Token;

// MARK: The first screen

const TITLE_FONT: Font = Font::sans(24.0, Font::MEDIUM);
const TITLE_LINE: f64 = 32.0;
/// From the title to the one line under it.
const TITLE_GAP: f64 = 8.0;
const DESCRIPTION_FONT: Font = Font::sans(14.0, Font::REGULAR);
const DESCRIPTION_LINE: f64 = 20.0;
/// From the description to the button.
const SPACE: f64 = 24.0;
/// The note line 12 pt under the button, when there is one.
const NOTE_GAP: f64 = 12.0;
const NOTE_FONT: Font = Font::sans(14.0, Font::REGULAR);
const NOTE_LINE: f64 = 20.0;

/// The first screen on a computer with no drive yet: a title, one line and one button, each
/// centred in the window.
#[derive(Clone, Debug, PartialEq)]
pub struct Onboarding {
    pub title: Line,
    pub description: Line,
    /// The button that opens the platform's folder picker; the folder picked becomes the drive.
    pub choose: Rect,
    pub choose_label: Line,
    /// Why the folder picked was refused, or that the one chosen cannot be used now.
    pub note: Option<Line>,
}

/// The first screen in a `width` by `height` view, with `note` under the button
/// (`selection::onboarding::note`; empty for none). Text is in `language`. Every line is as
/// wide as its words and centred across the view, and the whole is centred down it.
pub fn onboarding(
    width: f64,
    height: f64,
    note: &str,
    language: &str,
    measure: &mut Measure,
) -> Onboarding {
    let say = |key: strings::Key| strings::text(language, key);
    // The left edge of something `wide`, centred on a whole point.
    let centred = |wide: f64| ((width - wide) / 2.0).floor();
    let mut line = |words: &str, top: f64, tall: f64, font: Font, color: Token| {
        let wide = measure(words, font);
        Line::new(words, centred(wide), top, wide + 1.0, tall, font, color)
    };
    let noted = if note.is_empty() {
        0.0
    } else {
        NOTE_GAP + NOTE_LINE
    };
    let column_height = TITLE_LINE + TITLE_GAP + DESCRIPTION_LINE + SPACE + button::HEIGHT + noted;
    let mut y = ((height - column_height) / 2.0).floor();
    let title = line(
        say(key::ONBOARDING_TITLE),
        y,
        TITLE_LINE,
        TITLE_FONT,
        Token::Foreground,
    );
    y += TITLE_LINE + TITLE_GAP;
    let description = line(
        say(key::ONBOARDING_DESCRIPTION),
        y,
        DESCRIPTION_LINE,
        DESCRIPTION_FONT,
        Token::MutedForeground,
    );
    y += DESCRIPTION_LINE + SPACE;
    let mut choose_label = line(
        say(key::ONBOARDING_CHOOSE),
        y + button::WORDS_TOP,
        button::LINE,
        button::FONT,
        Token::CtaForeground,
    );
    let button_width = button::PADDING + (choose_label.frame.width - 1.0) + button::PADDING;
    let choose = Rect::new(centred(button_width), y, button_width, button::HEIGHT);
    choose_label.frame.x = choose.x + button::PADDING;
    y += button::HEIGHT;
    let note = (!note.is_empty())
        .then(|| line(note, y + NOTE_GAP, NOTE_LINE, NOTE_FONT, Token::Destructive));
    Onboarding {
        title,
        description,
        choose,
        choose_label,
        note,
    }
}

// MARK: The chip a drag carries

/// The chip is 26 pt tall and at most 224 pt wide. Its text sits 11 pt in and 5 pt down, with
/// 10 pt of padding and a 1 px border on each side.
const CHIP_MOST: f64 = 224.0;
const CHIP_HEIGHT: f64 = 26.0;
const CHIP_FONT: Font = Font::sans(12.0, Font::MEDIUM);
const CHIP_LINE: f64 = 16.0;
const CHIP_WORDS_LEFT: f64 = 11.0;
const CHIP_WORDS_TOP: f64 = 5.0;
const CHIP_ROOM: f64 = 22.0;
/// The pointer sits 14 pt right of and below the chip's top-left corner.
pub const CHIP_POINTER: f64 = 14.0;

/// What follows the pointer while files are dragged: a small panel-coloured chip.
#[derive(Clone, Debug, PartialEq)]
pub struct DragChip {
    pub width: f64,
    pub height: f64,
    pub words: Line,
}

/// The chip carrying `label` (`list::drag_label`): as wide as its text in whole points, up to
/// 224 pt.
pub fn drag_chip(label: &str, measure: &mut Measure) -> DragChip {
    let width = CHIP_MOST.min((measure(label, CHIP_FONT) + CHIP_ROOM).ceil());
    DragChip {
        width,
        height: CHIP_HEIGHT,
        words: Line::new(
            label,
            CHIP_WORDS_LEFT,
            CHIP_WORDS_TOP,
            width - CHIP_ROOM,
            CHIP_LINE,
            CHIP_FONT,
            Token::Foreground,
        ),
    }
}

// MARK: The window's own buttons

/// Close, minimise and zoom: the first 14 pt in and 14 pt down, each 20 pt after the last.
const WINDOW_BUTTONS_IN: f64 = 14.0;
const WINDOW_BUTTONS_DOWN: f64 = 14.0;
const WINDOW_BUTTON_PITCH: f64 = 20.0;

/// Where the top-left of the window's button `index` (0 close, 1 minimise, 2 zoom) sits, from the
/// window's top-left.
pub fn window_button(index: u32) -> (f64, f64) {
    (
        WINDOW_BUTTONS_IN + f64::from(index) * WINDOW_BUTTON_PITCH,
        WINDOW_BUTTONS_DOWN,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::typeset::test_measure::sevens;

    #[test]
    fn the_first_screen_is_a_title_a_line_and_a_button_each_centred() {
        let screen = onboarding(1280.0, 800.0, "", "en", &mut sevens);
        // 32 + 8 + 20 + 24 + 36 = 120 tall, so its top is 340 down. The title is 20 characters.
        assert_eq!(
            screen.title,
            Line::new(
                "Welcome to Northroom",
                570.0,
                340.0,
                141.0,
                32.0,
                TITLE_FONT,
                Token::Foreground
            )
        );
        // 34 characters.
        assert_eq!(
            screen.description,
            Line::new(
                "Choose the folder your work is in.",
                521.0,
                380.0,
                239.0,
                20.0,
                DESCRIPTION_FONT,
                Token::MutedForeground
            )
        );
        // "Choose Folder" is 13 characters: 16 + 91 + 16, centred on a whole point.
        assert_eq!(screen.choose, Rect::new(578.0, 424.0, 123.0, 36.0));
        assert_eq!(
            screen.choose_label,
            Line::new(
                "Choose Folder",
                594.0,
                432.0,
                92.0,
                20.0,
                button::FONT,
                Token::CtaForeground
            )
        );
        assert_eq!(screen.note, None);

        // A note takes 12 and its line of 20 under the button; the whole stays centred.
        let refused = onboarding(1280.0, 800.0, "Not this one.", "en", &mut sevens);
        assert_eq!(refused.title.frame.y, 324.0);
        assert_eq!(
            refused.note,
            Some(Line::new(
                "Not this one.",
                594.0,
                456.0,
                92.0,
                20.0,
                NOTE_FONT,
                Token::Destructive
            ))
        );
        assert_eq!(
            screen.title.text,
            strings::text("en", key::ONBOARDING_TITLE)
        );
    }

    #[test]
    fn a_drag_chip_fits_its_words_up_to_224_and_holds_the_pointer_14_in() {
        let chip = drag_chip("notes.md", &mut sevens);
        assert_eq!((chip.width, chip.height), (78.0, 26.0));
        assert_eq!(
            chip.words,
            Line::new(
                "notes.md",
                11.0,
                5.0,
                56.0,
                16.0,
                CHIP_FONT,
                Token::Foreground
            )
        );
        let long = drag_chip(&"a".repeat(60), &mut |text, _| text.len() as f64 * 6.5);
        assert_eq!((long.width, long.words.frame.width), (224.0, 202.0));
        assert_eq!(
            (drag_chip("x", &mut |_, _| 10.2).width),
            33.0,
            "whole points, rounded up"
        );
        assert_eq!(CHIP_POINTER, 14.0);
    }

    #[test]
    fn the_window_buttons_sit_14_in_and_14_down_20_apart() {
        assert_eq!(
            [window_button(0), window_button(1), window_button(2)],
            [(14.0, 14.0), (34.0, 14.0), (54.0, 14.0)]
        );
    }
}
