//! Layout of the first screen, the drag chip and the window's three buttons. Frames are in points
//! from the top-left of the view each is drawn in, and every line of text carries its font and
//! colour (`dialog::layout::Line`). A shell draws backgrounds and glyphs at these frames; text is
//! measured by the core's typesetter (`typeset`).

use crate::board::card::Font;
use crate::dialog::layout::{button, Line, Measure};
use crate::geometry::Rect;
use crate::strings::{self, key};
use crate::text::wrap::{self, Breaks};
use crate::theme::Token;

// MARK: The first screen

/// The column is 448 pt wide, centred in the window.
const COLUMN: f64 = 448.0;
const TITLE_FONT: Font = Font::sans(24.0, Font::MEDIUM);
const TITLE_LINE: f64 = 32.0;
/// From the title to the description, which always takes two 20 pt lines.
const TITLE_GAP: f64 = 8.0;
const DESCRIPTION_FONT: Font = Font::sans(14.0, Font::REGULAR);
const DESCRIPTION_LINE: f64 = 20.0;
const DESCRIPTION_HEIGHT: f64 = 40.0;
/// From the description to the buttons, and from what is above the footer to it.
const SPACE: f64 = 24.0;
/// "Use a folder I already have" is a text button 16 pt after Start, on the same line.
const CHOOSE_GAP: f64 = 16.0;
const CHOOSE_FONT: Font = Font::sans(14.0, Font::REGULAR);
/// The note line 12 pt under the buttons, when there is one.
const NOTE_GAP: f64 = 12.0;
const NOTE_FONT: Font = Font::sans(14.0, Font::REGULAR);
const NOTE_LINE: f64 = 20.0;
const FOOTER_FONT: Font = Font::sans(12.0, Font::REGULAR);
const FOOTER_LINE: f64 = 16.0;

/// The first screen on a computer with no drive yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Onboarding {
    pub title: Line,
    /// The description's lines, as a paragraph wraps them.
    pub description: Vec<Line>,
    /// Start, which makes `~/Northroom` the drive.
    pub start: Rect,
    pub start_label: Line,
    /// The text button that opens the platform's folder picker: as wide as its text, as tall as
    /// Start.
    pub choose: Rect,
    pub choose_label: Line,
    /// Why the folder picked was refused, or that the one chosen cannot be used now.
    pub note: Option<Line>,
    pub footer: Line,
}

/// The first screen in a `width` by `height` view, with `note` under the buttons
/// (`selection::onboarding::note`; empty for none). Text is in `language`, in a 448 pt column
/// centred both ways.
pub fn onboarding(
    width: f64,
    height: f64,
    note: &str,
    language: &str,
    measure: &mut Measure,
) -> Onboarding {
    let say = |key: strings::Key| strings::text(language, key);
    let x = (width - COLUMN) / 2.0;
    let noted = if note.is_empty() {
        0.0
    } else {
        NOTE_LINE + NOTE_GAP
    };
    let column_height = TITLE_LINE
        + TITLE_GAP
        + DESCRIPTION_HEIGHT
        + SPACE
        + button::HEIGHT
        + noted
        + SPACE
        + FOOTER_LINE;
    let mut y = (height - column_height) / 2.0;
    let title = Line::new(
        say(key::ONBOARDING_TITLE),
        x,
        y,
        COLUMN,
        TITLE_LINE,
        TITLE_FONT,
        Token::Foreground,
    );
    y += TITLE_LINE + TITLE_GAP;
    let description = wrap::lines(
        say(key::ONBOARDING_DESCRIPTION),
        COLUMN,
        Breaks::Words,
        &mut |line| measure(line, DESCRIPTION_FONT),
    )
    .iter()
    .enumerate()
    .map(|(index, line)| {
        let top = y + index as f64 * DESCRIPTION_LINE;
        Line::new(
            line,
            x,
            top,
            COLUMN,
            DESCRIPTION_LINE,
            DESCRIPTION_FONT,
            Token::MutedForeground,
        )
    })
    .collect();
    y += DESCRIPTION_HEIGHT + SPACE;
    let words = measure(say(key::ONBOARDING_START), button::FONT);
    let start = Rect::new(
        x,
        y,
        button::PADDING + words + button::PADDING,
        button::HEIGHT,
    );
    let start_label = Line::new(
        say(key::ONBOARDING_START),
        start.x + button::PADDING,
        y + button::WORDS_TOP,
        words + 1.0,
        button::LINE,
        button::FONT,
        Token::CtaForeground,
    );
    let words = measure(say(key::ONBOARDING_CHOOSE), CHOOSE_FONT);
    let choose = Rect::new(start.max_x() + CHOOSE_GAP, y, words + 1.0, button::HEIGHT);
    let choose_label = Line::new(
        say(key::ONBOARDING_CHOOSE),
        choose.x,
        y + button::WORDS_TOP,
        choose.width,
        button::LINE,
        CHOOSE_FONT,
        Token::Foreground,
    );
    y += button::HEIGHT;
    let note = (!note.is_empty()).then(|| {
        y += NOTE_GAP;
        let line = Line::new(note, x, y, COLUMN, NOTE_LINE, NOTE_FONT, Token::Destructive);
        y += NOTE_LINE;
        line
    });
    y += SPACE;
    let footer = Line::new(
        say(key::ONBOARDING_FOOTER),
        x,
        y,
        COLUMN,
        FOOTER_LINE,
        FOOTER_FONT,
        Token::MutedForeground,
    );
    Onboarding {
        title,
        description,
        start,
        start_label,
        choose,
        choose_label,
        note,
        footer,
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
    fn the_first_screen_is_a_column_448_wide_centred_with_its_note_only_when_there_is_one() {
        let screen = onboarding(1280.0, 800.0, "", "en", &mut sevens);
        // 32 + 8 + 40 + 24 + 36 + 24 + 16 = 180 tall, so its top is 310 down.
        assert_eq!(
            screen.title,
            Line::new(
                "Welcome to Northroom",
                416.0,
                310.0,
                448.0,
                32.0,
                TITLE_FONT,
                Token::Foreground
            )
        );
        assert_eq!(
            screen.description.len(),
            2,
            "the description wraps onto two lines"
        );
        assert_eq!(
            (screen.description[0].frame.y, screen.description[1].frame.y),
            (350.0, 370.0)
        );
        assert!(screen
            .description
            .iter()
            .all(|line| sevens(&line.text, DESCRIPTION_FONT) <= 448.0));
        // "Start" is 5 characters: 16 + 35 + 16.
        assert_eq!(screen.start, Rect::new(416.0, 414.0, 67.0, 36.0));
        assert_eq!(
            screen.start_label,
            Line::new(
                "Start",
                432.0,
                422.0,
                36.0,
                20.0,
                button::FONT,
                Token::CtaForeground
            )
        );
        // "Use a folder I already have" is 27 characters, 16 after Start, as tall as it.
        assert_eq!(screen.choose, Rect::new(499.0, 414.0, 190.0, 36.0));
        assert_eq!(
            screen.choose_label,
            Line::new(
                "Use a folder I already have",
                499.0,
                422.0,
                190.0,
                20.0,
                CHOOSE_FONT,
                Token::Foreground
            )
        );
        assert_eq!(screen.note, None);
        assert_eq!(screen.footer.frame, Rect::new(416.0, 474.0, 448.0, 16.0));

        // A note takes 12 and its line of 20 under the button; the column stays centred.
        let refused = onboarding(1280.0, 800.0, "Not this one.", "en", &mut sevens);
        assert_eq!(refused.title.frame.y, 294.0);
        assert_eq!(
            refused.note,
            Some(Line::new(
                "Not this one.",
                416.0,
                446.0,
                448.0,
                20.0,
                NOTE_FONT,
                Token::Destructive
            ))
        );
        assert_eq!(refused.footer.frame.y, 490.0);
        assert_eq!(
            screen.footer.text,
            "You can change this folder later in Settings → Account."
        );
        assert_eq!(
            onboarding(1280.0, 800.0, "", "en", &mut sevens).title.text,
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
