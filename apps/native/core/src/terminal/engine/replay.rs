//! A session's emulator written out as the bytes that bring a fresh emulator of the same size
//! to the same state: its scrollback and screen with their colours, where its cursor is, and
//! the modes its program set. Another computer following the session shows these first, then
//! the output as it comes (`Engine::follow`).
//!
//! A program on the alternate screen is written as that screen alone; what its shell showed
//! before it comes back only when the program leaves.

use super::{is_spacer, Listener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use std::fmt::Write;

/// Clears a following emulator: screen, scrollback, modes and colours.
const RESET: &str = "\x1bc";
const ALTERNATE_SCREEN: &str = "\x1b[?1049h";

/// The modes a program sets, with what sets each. All start unset in a fresh emulator.
const SET_MODES: [(TermMode, &str); 11] = [
    (TermMode::APP_CURSOR, "\x1b[?1h"),
    (TermMode::APP_KEYPAD, "\x1b="),
    (TermMode::INSERT, "\x1b[4h"),
    (TermMode::MOUSE_REPORT_CLICK, "\x1b[?1000h"),
    (TermMode::MOUSE_DRAG, "\x1b[?1002h"),
    (TermMode::MOUSE_MOTION, "\x1b[?1003h"),
    (TermMode::FOCUS_IN_OUT, "\x1b[?1004h"),
    (TermMode::UTF8_MOUSE, "\x1b[?1005h"),
    (TermMode::SGR_MOUSE, "\x1b[?1006h"),
    (TermMode::BRACKETED_PASTE, "\x1b[?2004h"),
    (TermMode::LINE_FEED_NEW_LINE, "\x1b[20h"),
];

/// The modes a fresh emulator starts with, with what unsets each.
const UNSET_MODES: [(TermMode, &str); 3] = [
    (TermMode::SHOW_CURSOR, "\x1b[?25l"),
    (TermMode::ALTERNATE_SCROLL, "\x1b[?1007l"),
    (TermMode::LINE_WRAP, "\x1b[?7l"),
];

/// How a cell is drawn: its colours and the flags that change its look.
#[derive(Clone, Copy, PartialEq)]
struct Pen {
    ink: Color,
    ground: Color,
    flags: Flags,
}

impl Pen {
    const LOOKS: Flags = Flags::BOLD
        .union(Flags::DIM)
        .union(Flags::ITALIC)
        .union(Flags::ALL_UNDERLINES)
        .union(Flags::INVERSE)
        .union(Flags::HIDDEN)
        .union(Flags::STRIKEOUT);

    /// A fresh emulator's pen.
    const PLAIN: Self = Self {
        ink: Color::Named(NamedColor::Foreground),
        ground: Color::Named(NamedColor::Background),
        flags: Flags::empty(),
    };

    fn of(cell: &Cell) -> Self {
        Self {
            ink: cell.fg,
            ground: cell.bg,
            flags: cell.flags.intersection(Self::LOOKS),
        }
    }

    /// The sequence that sets this pen whatever was set before.
    fn set(self, out: &mut String) {
        out.push_str("\x1b[0");
        for (flag, code) in [
            (Flags::BOLD, ";1"),
            (Flags::DIM, ";2"),
            (Flags::ITALIC, ";3"),
            (Flags::UNDERLINE, ";4"),
            (Flags::DOUBLE_UNDERLINE, ";21"),
            (Flags::UNDERCURL, ";4:3"),
            (Flags::DOTTED_UNDERLINE, ";4:4"),
            (Flags::DASHED_UNDERLINE, ";4:5"),
            (Flags::INVERSE, ";7"),
            (Flags::HIDDEN, ";8"),
            (Flags::STRIKEOUT, ";9"),
        ] {
            if self.flags.contains(flag) {
                out.push_str(code);
            }
        }
        colour(self.ink, Layer::Ink, out);
        colour(self.ground, Layer::Ground, out);
        out.push('m');
    }
}

/// Which of a cell's two colours.
#[derive(Clone, Copy)]
enum Layer {
    Ink,
    Ground,
}

impl Layer {
    /// The codes for the first eight colours, the bright eight, and any colour by number.
    fn codes(self) -> (u8, u8, u8) {
        match self {
            Self::Ink => (30, 90, 38),
            Self::Ground => (40, 100, 48),
        }
    }
}

/// Adds the code for `colour` on `layer`; nothing for the layer's own default.
fn colour(colour: Color, layer: Layer, out: &mut String) {
    let (plain, bright, any) = layer.codes();
    // Writing to a string cannot fail.
    let _ = match colour {
        Color::Spec(rgb) => write!(out, ";{any};2;{};{};{}", rgb.r, rgb.g, rgb.b),
        Color::Indexed(index) => write!(out, ";{any};5;{index}"),
        Color::Named(named) => match named as usize {
            index @ 0..=7 => write!(out, ";{}", usize::from(plain) + index),
            index @ 8..=15 => write!(out, ";{}", usize::from(bright) + index - 8),
            _ => Ok(()),
        },
    };
}

/// One line's cells up to its last one that is not blank, then whether it runs on into the
/// next line.
fn line(term: &Term<Listener>, line: Line, pen: &mut Pen, out: &mut String) -> bool {
    let row = &term.grid()[line];
    let columns = term.columns();
    let wraps = row[Column(columns - 1)].flags.contains(Flags::WRAPLINE);
    let blank = |cell: &Cell| cell.c == ' ' && Pen::of(cell) == Pen::PLAIN;
    // A line that runs on is written to its end, so the next one starts where it should.
    let shown = if wraps {
        columns
    } else {
        (0..columns)
            .rev()
            .find(|column| !blank(&row[Column(*column)]))
            .map_or(0, |last| last + 1)
    };
    for cell in (0..shown).map(|column| &row[Column(column)]) {
        if is_spacer(cell.flags) {
            continue;
        }
        let drawn = Pen::of(cell);
        if drawn != *pen {
            drawn.set(out);
            *pen = drawn;
        }
        out.push(cell.c);
        out.extend(cell.zerowidth().into_iter().flatten());
    }
    wraps
}

/// The modes `mode` differs in from a fresh emulator's.
fn modes(mode: TermMode, out: &mut String) {
    for (flag, set) in SET_MODES {
        if mode.contains(flag) {
            out.push_str(set);
        }
    }
    for (flag, unset) in UNSET_MODES {
        if !mode.contains(flag) {
            out.push_str(unset);
        }
    }
}

/// The cursor's shape as its style number: blinking first of each pair.
fn cursor_style(term: &Term<Listener>) -> usize {
    let style = term.cursor_style();
    let shape = match style.shape {
        CursorShape::Underline => 3,
        CursorShape::Beam => 5,
        _ => 1,
    };
    shape + usize::from(!style.blinking)
}

/// `term` as the bytes that bring a fresh emulator of its size to its state.
pub(super) fn of(term: &Term<Listener>) -> Vec<u8> {
    let mut out = String::from(RESET);
    let mode = *term.mode();
    if mode.contains(TermMode::ALT_SCREEN) {
        out.push_str(ALTERNATE_SCREEN);
    }
    let mut pen = Pen::PLAIN;
    let (top, bottom) = (term.topmost_line(), term.bottommost_line());
    for at in top.0..=bottom.0 {
        let wraps = line(term, Line(at), &mut pen, &mut out);
        if !wraps && at < bottom.0 {
            out.push_str("\r\n");
        }
    }
    let cursor = &term.grid().cursor;
    Pen::of(&cursor.template).set(&mut out);
    // Writing to a string cannot fail.
    let _ = write!(
        out,
        "\x1b[{};{}H\x1b[{} q",
        cursor.point.line.0 + 1,
        cursor.point.column.0 + 1,
        cursor_style(term)
    );
    modes(mode, &mut out);
    out.into_bytes()
}
