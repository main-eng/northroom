//! Layout for the drawn dialogs: the prompt or warning (`menus::Prompt`), the alert box used off
//! the Mac, and the corner notice.
//!
//! Frames are in points from the top-left of the covered view (the notice's text from the
//! notice's own). Each text line carries its font and colour and wraps with `wrap::lines`. A
//! shell draws backgrounds and controls at these frames and places native fields where they
//! say. Text is measured with `typeset`.

use crate::board::card::Font;
use crate::geometry::Rect;
use crate::menus::Prompt;
use crate::text::wrap::{self, Breaks};
use crate::theme::Token;

/// Width for a line bounded only by its own length.
pub const UNBOUNDED: f64 = f64::MAX;

/// One text line: its box (left, top, the width past which it ends in an ellipsis, and a height
/// the font is centred in), its font, and its theme colour at a fraction of that colour's
/// opacity.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub frame: Rect,
    pub font: Font,
    pub color: Token,
    pub opacity: f64,
}

impl Line {
    pub fn new(
        text: &str,
        x: f64,
        top: f64,
        width: f64,
        height: f64,
        font: Font,
        color: Token,
    ) -> Self {
        Self {
            text: text.to_owned(),
            frame: Rect::new(x, top, width, height),
            font,
            color,
            opacity: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Button {
    pub frame: Rect,
    pub words: Line,
}

/// A field: its drawn background and the frame of the native control on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldPlace {
    pub frame: Rect,
    pub control: Rect,
}

/// Measures text width in a font, as the shell sets it.
pub type Measure<'a> = dyn FnMut(&str, Font) -> f64 + 'a;

/// Wraps `text` at word breaks to `width` in `font` (`wrap::lines`).
fn broken(
    text: &str,
    width: f64,
    breaks: Breaks,
    font: Font,
    measure: &mut Measure,
) -> Vec<String> {
    wrap::lines(text, width, breaks, &mut |line| measure(line, font))
}

/// Text button metrics, shared by every dialog and the first screen: 36 pt tall, label in 14 pt
/// sans at weight 500 on a 20 pt line, inset 16 pt at the sides and 8 pt from the top.
pub(crate) mod button {
    use crate::board::card::Font;

    pub(crate) const HEIGHT: f64 = 36.0;
    pub(crate) const PADDING: f64 = 16.0;
    pub(crate) const FONT: Font = Font::sans(14.0, Font::MEDIUM);
    pub(crate) const LINE: f64 = 20.0;
    pub(crate) const WORDS_TOP: f64 = 8.0;
}

// MARK: All dialogs

/// Minimum gap between a panel and the window's sides.
const PANEL_MARGIN: f64 = 16.0;
/// The panel's 1 px border and 24 pt padding.
const EDGE: f64 = 25.0;
const TITLE_FONT: Font = Font::sans(18.0, Font::SEMIBOLD);
const TEXT_FONT: Font = Font::sans(14.0, Font::REGULAR);
const TEXT_LINE: f64 = 20.0;
/// A one-line field is as tall as a button, with text 8 pt from the top.
const CONTROL: f64 = button::HEIGHT;
const WORDS_DOWN: f64 = 8.0;
/// Gap between a dialog's parts.
const SPACE: f64 = 16.0;
/// The close mark: 16 pt square, 17 pt from the panel's top and 33 pt from its right.
const CLOSE: f64 = 16.0;
const CLOSE_TOP: f64 = 17.0;
const CLOSE_RIGHT: f64 = 33.0;

/// Centres a `width` × `height` panel in a `view_width` × `view_height` view, on whole points.
fn centred(view_width: f64, view_height: f64, width: f64, height: f64) -> Rect {
    Rect::new(
        ((view_width - width) / 2.0).round(),
        ((view_height - height) / 2.0).round(),
        width,
        height,
    )
}

fn close_mark(panel: Rect) -> Rect {
    Rect::new(
        panel.max_x() - CLOSE_RIGHT,
        panel.y + CLOSE_TOP,
        CLOSE,
        CLOSE,
    )
}

// MARK: Prompt and warning dialog

const PROMPT_MAX: f64 = 512.0;
/// Title line height: the font's own in a prompt, 28 pt in a warning.
const TITLE_LINE: f64 = 18.0;
const WARNING_TITLE_LINE: f64 = 28.0;
/// Gap from the title to the sentence, and from Cancel to the action.
const SENTENCE_GAP: f64 = 8.0;
const BUTTON_GAP: f64 = 8.0;
/// Field text: inset 12 pt at the sides, on a 20 px line 8 pt from the top.
const FIELD_INSET: f64 = 12.0;

#[derive(Clone, Debug, PartialEq)]
pub struct PromptLayout {
    pub panel: Rect,
    /// A prompt has a close mark in its corner; a warning has none.
    pub close: Option<Rect>,
    pub title: Line,
    pub message: Vec<Line>,
    pub field: Option<FieldPlace>,
    pub cancel: Button,
    pub action: Button,
}

/// Lays out `prompt` over a `width` × `height` view, with the action label in `action_color`
/// (`dialog::keys::action_text_color`).
pub fn prompt(
    prompt: &Prompt,
    width: f64,
    height: f64,
    action_color: Token,
    measure: &mut Measure,
) -> PromptLayout {
    let panel_width = PROMPT_MAX.min(width - 2.0 * PANEL_MARGIN);
    let inner = panel_width - 2.0 * EDGE;
    let title_line = if prompt.destructive {
        WARNING_TITLE_LINE
    } else {
        TITLE_LINE
    };
    let message = prompt
        .message
        .as_deref()
        .map(|message| broken(message, inner, Breaks::Words, TEXT_FONT, measure));
    let mut panel_height = EDGE + title_line;
    if let Some(lines) = &message {
        panel_height += SENTENCE_GAP + lines.len() as f64 * TEXT_LINE;
    }
    if prompt.value.is_some() {
        panel_height += SPACE + CONTROL;
    }
    panel_height += SPACE + button::HEIGHT + EDGE;
    let panel = centred(width, height, panel_width, panel_height);
    let left = panel.x + EDGE;
    let mut y = panel.y + EDGE;
    let title = Line::new(
        &prompt.title,
        left,
        y,
        inner,
        title_line,
        TITLE_FONT,
        Token::Foreground,
    );
    y += title_line;
    let message = message.map_or_else(Vec::new, |lines| {
        y += SENTENCE_GAP;
        let top = y;
        y += lines.len() as f64 * TEXT_LINE;
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                Line::new(
                    line,
                    left,
                    top + index as f64 * TEXT_LINE,
                    inner,
                    TEXT_LINE,
                    TEXT_FONT,
                    Token::MutedForeground,
                )
            })
            .collect()
    });
    let field = prompt.value.as_ref().map(|_| {
        y += SPACE;
        let frame = Rect::new(left, y, inner, CONTROL);
        y += CONTROL;
        FieldPlace {
            frame,
            control: Rect::new(
                left + FIELD_INSET,
                frame.y + WORDS_DOWN,
                inner - 2.0 * FIELD_INSET,
                TEXT_LINE,
            ),
        }
    });
    y += SPACE;
    let action_width = (measure(prompt.action, button::FONT) + 2.0 * button::PADDING).ceil();
    // Cancel is an outline button: its 1 px border sits outside its 16 px padding.
    let cancel_width = (measure(prompt.cancel, button::FONT) + 2.0 * button::PADDING + 2.0).ceil();
    let action = Rect::new(
        panel.max_x() - EDGE - action_width,
        y,
        action_width,
        button::HEIGHT,
    );
    let cancel = Rect::new(
        action.x - BUTTON_GAP - cancel_width,
        y,
        cancel_width,
        button::HEIGHT,
    );
    let button = |frame: Rect, words: &str, color: Token| Button {
        frame,
        words: Line::new(
            words,
            frame.x + button::PADDING,
            frame.y + button::WORDS_TOP,
            frame.width,
            button::LINE,
            button::FONT,
            color,
        ),
    };
    PromptLayout {
        panel,
        close: (!prompt.destructive).then(|| close_mark(panel)),
        title,
        message,
        field,
        cancel: button(cancel, prompt.cancel, Token::Foreground),
        action: button(action, prompt.action, action_color),
    }
}

// MARK: Alert box off the Mac

/// The app's own alert box, for platforms whose native one won't keep Return on a chosen button
/// (such as the update prompt). Drawn like the warning dialog: the message as title, the detail
/// as sentence, buttons in order along the foot, the Return button filled with the accent and
/// the rest outlined.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertLayout {
    pub panel: Rect,
    pub title: Line,
    pub message: Vec<Line>,
    pub buttons: Vec<Button>,
}

/// Lays out an alert with `message`, `detail` and `buttons` (an empty list gives a single `ok`)
/// over a `width` × `height` view. Return presses `return_button`, labelled in `default_color`
/// (`dialog::keys::action_text_color`).
#[allow(clippy::too_many_arguments)]
pub fn alert(
    message: &str,
    detail: &str,
    buttons: &[&str],
    ok: &str,
    return_button: usize,
    width: f64,
    height: f64,
    default_color: Token,
    measure: &mut Measure,
) -> AlertLayout {
    let buttons = if buttons.is_empty() {
        &[ok][..]
    } else {
        buttons
    };
    let panel_width = PROMPT_MAX.min(width - 2.0 * PANEL_MARGIN);
    let inner = panel_width - 2.0 * EDGE;
    let lines = broken(detail, inner, Breaks::Words, TEXT_FONT, measure);
    let mut panel_height = EDGE + WARNING_TITLE_LINE;
    if !lines.is_empty() {
        panel_height += SENTENCE_GAP + lines.len() as f64 * TEXT_LINE;
    }
    panel_height += SPACE + button::HEIGHT + EDGE;
    let panel = centred(width, height, panel_width, panel_height);
    let left = panel.x + EDGE;
    let mut y = panel.y + EDGE;
    let title = Line::new(
        message,
        left,
        y,
        inner,
        WARNING_TITLE_LINE,
        TITLE_FONT,
        Token::Foreground,
    );
    y += WARNING_TITLE_LINE;
    if !lines.is_empty() {
        y += SENTENCE_GAP;
    }
    let message = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            Line::new(
                line,
                left,
                y + index as f64 * TEXT_LINE,
                inner,
                TEXT_LINE,
                TEXT_FONT,
                Token::MutedForeground,
            )
        })
        .collect();
    y += lines.len() as f64 * TEXT_LINE + SPACE;
    // An outlined button's 1 px border sits outside its 16 px padding, as Cancel's does.
    let widths: Vec<f64> = buttons
        .iter()
        .enumerate()
        .map(|(place, words)| {
            let border = if place == return_button { 0.0 } else { 2.0 };
            (measure(words, button::FONT) + 2.0 * button::PADDING + border).ceil()
        })
        .collect();
    let row: f64 = widths.iter().sum::<f64>() + BUTTON_GAP * (widths.len() - 1) as f64;
    let mut x = panel.max_x() - EDGE - row;
    let buttons = buttons
        .iter()
        .zip(&widths)
        .enumerate()
        .map(|(place, (words, &button_width))| {
            let frame = Rect::new(x, y, button_width, button::HEIGHT);
            x += button_width + BUTTON_GAP;
            let color = if place == return_button {
                default_color
            } else {
                Token::Foreground
            };
            Button {
                frame,
                words: Line::new(
                    words,
                    frame.x + button::PADDING,
                    frame.y + button::WORDS_TOP,
                    frame.width,
                    button::LINE,
                    button::FONT,
                    color,
                ),
            }
        })
        .collect();
    AlertLayout {
        panel,
        title,
        message,
        buttons,
    }
}

// MARK: Notice

/// A notice is 356 pt wide, 16 pt from the window's right and bottom, with text inset 16 pt at
/// the sides and 12 pt from the top.
const TOAST_WIDTH: f64 = 356.0;
const TOAST_MARGIN: f64 = 16.0;
const TOAST_INSET: f64 = 16.0;
const TOAST_PADDING: f64 = 12.0;
const TOAST_DETAIL_FONT: Font = Font::sans(12.0, Font::REGULAR);
const TOAST_DETAIL_LINE: f64 = 16.0;
/// Gap from the title line to the reason line.
const TOAST_DETAIL_GAP: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct ToastLayout {
    /// In the window's content, from its top-left.
    pub frame: Rect,
    /// From the notice's own top-left.
    pub title: Line,
    pub detail: Vec<Line>,
}

/// Lays out a notice with `title` and the reason in `detail` in a `width` × `height` window.
pub fn toast(
    title: &str,
    detail: &str,
    width: f64,
    height: f64,
    measure: &mut Measure,
) -> ToastLayout {
    let room = TOAST_WIDTH - 2.0 * TOAST_INSET;
    let lines = broken(detail, room, Breaks::Words, TOAST_DETAIL_FONT, measure);
    let detail_top = TOAST_PADDING + TEXT_LINE + TOAST_DETAIL_GAP;
    let detail_height = if lines.is_empty() {
        0.0
    } else {
        TOAST_DETAIL_GAP + lines.len() as f64 * TOAST_DETAIL_LINE
    };
    let toast_height = TOAST_PADDING + TEXT_LINE + detail_height + TOAST_PADDING;
    ToastLayout {
        frame: Rect::new(
            width - TOAST_WIDTH - TOAST_MARGIN,
            height - toast_height - TOAST_MARGIN,
            TOAST_WIDTH,
            toast_height,
        ),
        title: Line::new(
            title,
            TOAST_INSET,
            TOAST_PADDING,
            room,
            TEXT_LINE,
            button::FONT,
            Token::Foreground,
        ),
        detail: lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                Line::new(
                    line,
                    TOAST_INSET,
                    detail_top + index as f64 * TOAST_DETAIL_LINE,
                    room,
                    TOAST_DETAIL_LINE,
                    TOAST_DETAIL_FONT,
                    Token::MutedForeground,
                )
            })
            .collect(),
    }
}

// MARK: Control size

/// A shell control's size, as its view reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::typeset::test_measure::sevens;

    fn naming(message: Option<&str>) -> Prompt {
        Prompt {
            title: "New Folder".into(),
            message: message.map(str::to_owned),
            value: Some("Untitled".into()),
            action: "Save",
            cancel: "Cancel",
            destructive: false,
            secret: false,
        }
    }

    #[test]
    fn a_naming_dialog_is_at_most_512_wide_and_centred_with_its_field_over_the_buttons() {
        let placed = prompt(
            &naming(None),
            1280.0,
            800.0,
            Token::PrimaryForeground,
            &mut sevens,
        );
        // 25 + 18, 16 + 36 of field, 16 + 36 of buttons, 25.
        assert_eq!(placed.panel, Rect::new(384.0, 314.0, 512.0, 172.0));
        assert_eq!(placed.close, Some(Rect::new(863.0, 331.0, 16.0, 16.0)));
        assert_eq!(
            placed.title,
            Line::new(
                "New Folder",
                409.0,
                339.0,
                462.0,
                18.0,
                TITLE_FONT,
                Token::Foreground
            )
        );
        let field = placed.field.expect("a field");
        assert_eq!(field.frame, Rect::new(409.0, 373.0, 462.0, 36.0));
        assert_eq!(field.control, Rect::new(421.0, 381.0, 438.0, 20.0));
        // Save: 28 of words and 32 of padding; Cancel 42 and 34, 8 before it.
        assert_eq!(
            placed.action.frame,
            Rect::new(871.0 - 60.0, 425.0, 60.0, 36.0)
        );
        assert_eq!(
            placed.cancel.frame,
            Rect::new(811.0 - 8.0 - 76.0, 425.0, 76.0, 36.0)
        );
        assert_eq!(
            placed.cancel.words,
            Line::new(
                "Cancel",
                743.0,
                433.0,
                76.0,
                20.0,
                button::FONT,
                Token::Foreground
            )
        );
        assert_eq!(placed.action.words.color, Token::PrimaryForeground);
    }

    #[test]
    fn a_warning_has_no_close_mark_a_taller_title_and_its_sentence_broken_to_the_panel() {
        let warning = Prompt {
            title: "Move to Trash?".into(),
            message: Some("word ".repeat(20)),
            value: None,
            action: "Move to Trash",
            cancel: "Cancel",
            destructive: true,
            secret: false,
        };
        // A narrow window leaves 16 either side.
        let placed = prompt(&warning, 400.0, 600.0, Token::Ink, &mut sevens);
        assert_eq!(placed.panel.width, 368.0);
        assert!(placed.close.is_none() && placed.field.is_none());
        assert_eq!(placed.title.frame.height, 28.0);
        // 318 wide: nine words on a line, each 28 and a space of 7.
        let lines: Vec<&str> = placed
            .message
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(placed.message[1].frame.y - placed.message[0].frame.y, 20.0);
        assert_eq!(
            placed.message[0].frame.y,
            placed.panel.y + 25.0 + 28.0 + 8.0
        );
        assert_eq!(
            placed.panel.height,
            25.0 + 28.0 + 8.0 + 60.0 + 16.0 + 36.0 + 25.0
        );
        assert_eq!(
            placed.action.frame.y,
            placed.panel.y + 25.0 + 28.0 + 8.0 + 60.0 + 16.0
        );
    }

    #[test]
    fn an_alert_puts_its_buttons_in_order_at_the_foot_with_returns_filled() {
        let buttons = ["Copy JSON", "Copy TOML", "Done"];
        let placed = alert(
            "Connect",
            &"word ".repeat(20),
            &buttons,
            "OK",
            2,
            1280.0,
            800.0,
            Token::PrimaryForeground,
            &mut sevens,
        );
        assert_eq!(placed.panel.width, 512.0);
        // 462 wide: thirteen words of 28 and a space of 7 on a line, two lines.
        assert_eq!(placed.message.len(), 2);
        assert_eq!(
            placed.panel.height,
            25.0 + 28.0 + 8.0 + 40.0 + 16.0 + 36.0 + 25.0
        );
        // Copy JSON and Copy TOML: 63 of words, 32 of padding, 2 of border; Done 28 and 32.
        let frames: Vec<Rect> = placed.buttons.iter().map(|button| button.frame).collect();
        let foot = placed.panel.y + 25.0 + 28.0 + 8.0 + 40.0 + 16.0;
        let right = placed.panel.max_x() - 25.0;
        assert_eq!(frames[2], Rect::new(right - 60.0, foot, 60.0, 36.0));
        assert_eq!(
            frames[1],
            Rect::new(right - 60.0 - 8.0 - 97.0, foot, 97.0, 36.0)
        );
        assert_eq!(frames[0].max_x(), frames[1].x - 8.0);
        assert_eq!(placed.buttons[2].words.color, Token::PrimaryForeground);
        assert_eq!(placed.buttons[0].words.color, Token::Foreground);
        // No buttons is the platform's OK, with Return; no detail, no sentence.
        let bare = alert(
            "Missing",
            "",
            &[],
            "OK",
            0,
            1280.0,
            800.0,
            Token::PrimaryForeground,
            &mut sevens,
        );
        assert_eq!(bare.buttons.len(), 1);
        assert_eq!(bare.buttons[0].words.text, "OK");
        assert!(bare.message.is_empty());
        assert_eq!(bare.panel.height, 25.0 + 28.0 + 16.0 + 36.0 + 25.0);
    }

    #[test]
    fn a_toast_sits_at_the_lower_right_and_grows_with_its_reason() {
        let bare = toast("Failed", "", 1280.0, 800.0, &mut sevens);
        assert_eq!(
            bare.frame,
            Rect::new(1280.0 - 356.0 - 16.0, 800.0 - 44.0 - 16.0, 356.0, 44.0)
        );
        assert!(bare.detail.is_empty());
        let placed = toast("Failed", &"why ".repeat(30), 1280.0, 800.0, &mut sevens);
        // 324 wide: eleven words of 28 on a line, three lines of 16 and 2 above them.
        assert_eq!(placed.detail.len(), 3);
        assert_eq!(placed.frame.height, 44.0 + 2.0 + 48.0);
        assert_eq!(
            placed.title,
            Line::new(
                "Failed",
                16.0,
                12.0,
                324.0,
                20.0,
                button::FONT,
                Token::Foreground
            )
        );
        assert_eq!(
            placed.detail[2].frame,
            Rect::new(16.0, 34.0 + 32.0, 324.0, 16.0)
        );
    }
}
