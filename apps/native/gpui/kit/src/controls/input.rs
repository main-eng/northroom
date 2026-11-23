//! A text field, single-line or wrapped (the message box), built on GPUI's
//! `EntityInputHandler`. Typing and input methods arrive through `NSTextInputClient` (marked
//! text underlined, candidate window at the caret), alongside pointer and key selection, the
//! clipboard, and undo and redo (`edit::Editing`). It draws on one of three backgrounds:
//!
//! - `Ground::Settings`: the settings input fill, 6 px corners, 12 px text inset 9 pt, and a
//!   1 px ring-colour border while focused.
//! - `Ground::Dialog`: a dialog field. A warning-colour border while invalid, otherwise a ring
//!   border plus a 2 px ring at 30% while focused.
//! - `Ground::Bare`: text only, on the owner's background (the amount field, the colour
//!   picker's hex and channels).
//!
//! The core sets the text (`text::set`) and each glyph paints where it was placed. The caret,
//! selection, hit testing and input method rects all use the same lines (`FieldLine`).
//!
//! The placeholder shows whenever the field is empty, focused or not, as in AppKit. Selected
//! text uses the accent. With `Keys::Dialog`, Tab, ⇧Tab, Return and Escape go to the owner as
//! `Command`s, and it hands back the ones the core passes on (`pass_on`).

use super::Drawn;
use super::edit::*;
use super::{Line, soft_ring, strength};
use crate::core::inset;
use crate::draw::{rounded, stroke};
use crate::platform::{read_clipboard, write_clipboard};
use crate::text::{self, Run, browser_baseline};
use crate::theme::{Face, Token, color};
use gpui::{
    App, Bounds, ContentMask, Context, CursorStyle, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, Role, SharedString, StatefulInteractiveElement, Styled, UTF16Selection, Window,
    actions, canvas, point, prelude::FluentBuilder, px, size,
};
use northroom_core::text::typeface::Placed;
use std::ops::Range;
use std::time::Duration;

actions!(
    kit_field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Top,
        Bottom,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Tab,
        BackTab,
        Return,
        Escape,
        // Edit menu commands AppKit text fields answer: Paste and Match Style, Delete (the
        // selection), and Edit ▸ Speech ▸ Start and Stop Speaking.
        PasteAndMatchStyle,
        DeleteSelection,
        StartSpeaking,
        StopSpeaking,
    ]
);

pub const CONTEXT: &str = "KitField";

/// Key bindings scoped to the field's context.
pub fn bindings() -> Vec<KeyBinding> {
    let context = Some(CONTEXT);
    vec![
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("up", Up, context),
        KeyBinding::new("down", Down, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("cmd-left", Home, context),
        KeyBinding::new("cmd-right", End, context),
        KeyBinding::new("home", Top, context),
        KeyBinding::new("end", Bottom, context),
        KeyBinding::new("cmd-up", Top, context),
        KeyBinding::new("cmd-down", Bottom, context),
        KeyBinding::new("cmd-shift-left", SelectHome, context),
        KeyBinding::new("cmd-shift-right", SelectEnd, context),
        KeyBinding::new("cmd-a", SelectAll, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-x", Cut, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("cmd-z", Undo, context),
        KeyBinding::new("cmd-shift-z", Redo, context),
        KeyBinding::new("tab", Tab, context),
        KeyBinding::new("shift-tab", BackTab, context),
        KeyBinding::new("enter", Return, context),
        KeyBinding::new("escape", Escape, context),
    ]
}

/// One line that scrolls horizontally to keep the caret visible, or lines wrapped at the
/// field's width, each `line_height` tall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    OneLine,
    Wrapped { line_height: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Centre,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    Settings,
    Dialog { wrong: bool },
    Bare,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Placeholder {
    Nothing,
    /// Muted text where the field's text goes.
    Words(String),
    /// A line placed by a dialog layout.
    Placed(Line),
}

/// Who handles Tab, ⇧Tab, Return and Escape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keys {
    /// The field: Return commits (or inserts a newline when wrapped), Tab and ⇧Tab move focus.
    Own,
    /// The dialog, through `Command` events.
    Dialog,
}

/// The user changed the text.
pub struct Changed(pub String);
/// The text was committed: Return in a single-line field, or focus left the field.
pub struct Committed(pub String);
/// Return in a single-line field, which submits like a form's default button. Emitted before
/// the `Committed` from the same Return.
pub struct Submitted(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Tab,
    BackTab,
    Return,
    Escape,
}

/// Caret blink phase length, matching `NSTextView`.
const BLINK: Duration = Duration::from_millis(530);
/// How far the caret stays from a single-line field's edge while scrolling.
const SCROLL_MARGIN: f32 = 0.0;

pub struct Field {
    pub editing: Editing,
    focus: FocusHandle,
    /// The field's frame in the window.
    pub frame: Bounds<Pixels>,
    /// The text area: the top of the first line box and the available room.
    pub words: Bounds<Pixels>,
    pub face: Face,
    /// The text's theme colour.
    pub ink: Token,
    pub shape: Shape,
    pub align: Align,
    pub ground: Ground,
    pub placeholder: Placeholder,
    pub keys: Keys,
    pub enabled: bool,
    /// Whether the caret blinks. Off for scripted screenshots, which keep it shown.
    pub blinks: bool,
    /// The accessible label; empty means none, as for an untitled AppKit field.
    label: SharedString,
    /// The accessibility identifier (`NSView.identifier`), if not empty. A setting's field
    /// uses the setting's name.
    pub identifier: SharedString,
    scroll: f32,
    selecting: bool,
    caret_shown: bool,
    blink_epoch: usize,
}

impl EventEmitter<Changed> for Field {}
impl EventEmitter<Committed> for Field {}
impl EventEmitter<Submitted> for Field {}
impl EventEmitter<Command> for Field {}

impl Field {
    pub fn new(label: &str, content: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |field, _, cx| field.start_blinking(cx))
            .detach();
        cx.on_blur(&focus, window, |field, window, cx| {
            // GPUI blurs when the window goes behind another, but AppKit keeps editing then.
            // Only a focus change inside the window ends editing.
            if field.focus.is_focused(window) {
                return;
            }
            field.editing.marked = None;
            cx.emit(Committed(field.editing.content.clone()));
            cx.notify();
        })
        .detach();
        Self {
            editing: Editing::new(content),
            focus,
            frame: Bounds::default(),
            words: Bounds::default(),
            face: Face::sans(12.0, 400.0),
            ink: Token::Foreground,
            shape: Shape::OneLine,
            align: Align::Left,
            ground: Ground::Settings,
            placeholder: Placeholder::Nothing,
            keys: Keys::Own,
            enabled: true,
            blinks: true,
            label: SharedString::from(label.to_owned()),
            identifier: SharedString::default(),
            scroll: 0.0,
            selecting: false,
            caret_shown: true,
            blink_epoch: 0,
        }
    }

    /// The text rect for a settings input at `frame`: inset 9 pt from the sides, 5 pt down.
    pub fn settings(frame: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: point(frame.origin.x + px(9.0), frame.origin.y + px(5.0)),
            size: size(frame.size.width - px(18.0), px(16.0)),
        }
    }

    /// Places the field at `frame` and its text area at `words`.
    pub fn place(&mut self, frame: Bounds<Pixels>, words: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        self.words = words;
        cx.notify();
    }

    /// Replaces the text from outside (the core changed it).
    pub fn set_content(&mut self, content: &str, cx: &mut Context<Self>) {
        if self.editing.content != content {
            self.editing.replace_all(content);
            cx.notify();
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    fn start_blinking(&mut self, cx: &mut Context<Self>) {
        self.caret_shown = true;
        self.blink_epoch += 1;
        if !self.blinks {
            return;
        }
        let epoch = self.blink_epoch;
        cx.spawn(async move |field, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let going = field.update(cx, |field, cx| {
                    if field.blink_epoch != epoch {
                        return false;
                    }
                    field.caret_shown = !field.caret_shown;
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// After a change: shows the caret, restarts the blink and notifies the platform.
    fn changed(&mut self, words: bool, window: &mut Window, cx: &mut Context<Self>) {
        if words {
            window.notify_text_input_changed(&self.focus);
            cx.emit(Changed(self.editing.content.clone()));
        } else {
            window.notify_text_selection_changed(&self.focus);
        }
        if self.focus.is_focused(window) {
            self.start_blinking(cx);
        }
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.move_to(offset);
        self.changed(false, window, cx);
    }

    fn select_to(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.select_to(offset);
        self.changed(false, window, cx);
    }

    fn edit(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        group: Group,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let text = match self.shape {
            // A single-line field joins lines with a space, as `NSTextField` does.
            Shape::OneLine => text.replace(['\r', '\n'], " "),
            Shape::Wrapped { .. } => text.replace('\r', ""),
        };
        self.editing.edit(range, &text, group);
        self.changed(true, window, cx);
    }

    // MARK: Keys

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        let editing = &self.editing;
        let to = if editing.selected.is_empty() {
            previous_grapheme(&editing.content, editing.caret())
        } else {
            editing.selected.start
        };
        self.move_to(to, window, cx);
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        let editing = &self.editing;
        let to = if editing.selected.is_empty() {
            next_grapheme(&editing.content, editing.caret())
        } else {
            editing.selected.end
        };
        self.move_to(to, window, cx);
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(-1);
        self.move_to(to, window, cx);
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(1);
        self.move_to(to, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        let to = previous_grapheme(&self.editing.content, self.editing.caret());
        self.select_to(to, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        let to = next_grapheme(&self.editing.content, self.editing.caret());
        self.select_to(to, window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(-1);
        self.select_to(to, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(1);
        self.select_to(to, window, cx);
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        let to = previous_word(&self.editing.content, self.editing.caret());
        self.move_to(to, window, cx);
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        let to = next_word(&self.editing.content, self.editing.caret());
        self.move_to(to, window, cx);
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = previous_word(&self.editing.content, self.editing.caret());
        self.select_to(to, window, cx);
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to = next_word(&self.editing.content, self.editing.caret());
        self.select_to(to, window, cx);
    }

    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.line_end(false);
        self.move_to(to, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.line_end(true);
        self.move_to(to, window, cx);
    }

    fn select_home(&mut self, _: &SelectHome, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.line_end(false);
        self.select_to(to, window, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.line_end(true);
        self.select_to(to, window, cx);
    }

    fn top(&mut self, _: &Top, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
    }

    fn bottom(&mut self, _: &Bottom, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.editing.content.len(), window, cx);
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.select_all();
        self.changed(false, window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.selected.is_empty() {
            let before = previous_grapheme(&self.editing.content, self.editing.caret());
            if before == self.editing.caret() {
                window.play_system_bell();
                return;
            }
            self.editing.select_to(before);
        }
        self.edit(None, "", Group::Alone, window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.selected.is_empty() {
            let after = next_grapheme(&self.editing.content, self.editing.caret());
            if after == self.editing.caret() {
                window.play_system_bell();
                return;
            }
            self.editing.select_to(after);
        }
        self.edit(None, "", Group::Alone, window, cx);
    }

    /// Available only with a selection (`render`).
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        write_clipboard(self.editing.selected_words(), cx);
    }

    /// Available only with a selection (`render`).
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        write_clipboard(self.editing.selected_words(), cx);
        self.edit(None, "", Group::Alone, window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = read_clipboard(cx) {
            self.edit(None, &text, Group::Alone, window, cx);
        }
    }

    /// Fields always paste plain text.
    fn paste_and_match_style(
        &mut self,
        _: &PasteAndMatchStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste(&Paste, window, cx);
    }

    /// Deletes the selection without copying it. Available only with a selection (`render`).
    fn delete_selection(
        &mut self,
        _: &DeleteSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.edit(None, "", Group::Alone, window, cx);
    }

    /// Speaks the selection, or all the text when nothing is selected.
    fn start_speaking(&mut self, _: &StartSpeaking, _: &mut Window, _: &mut Context<Self>) {
        let words = if self.editing.selected.is_empty() {
            self.editing.content.as_str()
        } else {
            self.editing.selected_words()
        };
        crate::platform::start_speaking(words);
    }

    fn stop_speaking(&mut self, _: &StopSpeaking, _: &mut Window, _: &mut Context<Self>) {
        crate::platform::stop_speaking();
    }

    /// Available only when there is something to undo (`render`).
    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.undo();
        self.changed(true, window, cx);
    }

    /// Available only when there is something to redo (`render`).
    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.redo();
        self.changed(true, window, cx);
    }

    fn command(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match self.keys {
            Keys::Dialog => cx.emit(command),
            Keys::Own => self.pass_on(command, window, cx),
        }
    }

    /// Handles a dialog key in the field itself, when the dialog passes it on.
    pub fn pass_on(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        match (command, self.shape) {
            (Command::Return, Shape::Wrapped { .. }) => {
                self.edit(None, "\n", Group::Typing, window, cx)
            }
            (Command::Return, Shape::OneLine) => {
                cx.emit(Submitted(self.editing.content.clone()));
                cx.emit(Committed(self.editing.content.clone()));
            }
            (Command::Tab, _) => window.focus_next(cx),
            (Command::BackTab, _) => window.focus_prev(cx),
            (Command::Escape, _) => {}
        }
    }

    // MARK: Layout

    /// The text laid out as the core sets it (`text::set`): one line, or lines wrapped at the
    /// text width, with a line after a trailing newline and one line when empty. Never cached,
    /// because an input method asks for positions right after an edit, before the next frame.
    fn lines(&self) -> Vec<FieldLine> {
        let content = &self.editing.content;
        let room = f32::from(self.words.size.width);
        let (width, wraps) = match self.shape {
            Shape::Wrapped { .. } if room > 0.0 => (room, true),
            _ => (f32::INFINITY, false),
        };
        let run = Run {
            text: content.clone(),
            face: self.face,
            color: Hsla::default(),
            underline: None,
            strike: false,
            code: false,
            space: 0.0,
            tracking: 0.0,
        };
        let mut lines: Vec<FieldLine> = text::set(&[run], width, wraps)
            .into_iter()
            .filter_map(|line| {
                let piece = line.pieces.into_iter().next()?;
                Some(FieldLine {
                    start: piece.start,
                    end: piece.end,
                    width: line.width,
                    glyphs: piece
                        .glyphs
                        .into_iter()
                        .map(|glyph| Placed {
                            x: f64::from(piece.x) + glyph.x,
                            ..glyph
                        })
                        .collect(),
                })
            })
            .collect();
        if lines.is_empty() || content.ends_with('\n') {
            lines.push(FieldLine {
                start: content.len(),
                end: content.len(),
                width: 0.0,
                glyphs: Vec::new(),
            });
        }
        lines
    }

    fn line_height(&self) -> f32 {
        match self.shape {
            Shape::OneLine => f32::from(self.words.size.height),
            Shape::Wrapped { line_height } => line_height,
        }
    }

    /// The line containing `index`. A wrap point belongs to the next line.
    fn line_of(lines: &[FieldLine], index: usize) -> usize {
        lines
            .iter()
            .rposition(|line| line.start <= index)
            .unwrap_or(0)
    }

    /// The position of `index` from the text area's top-left, before scrolling.
    fn position(&self, lines: &[FieldLine], index: usize) -> Point<Pixels> {
        let row = Self::line_of(lines, index);
        point(
            px(self.indent(lines) + lines[row].x_of(index)),
            px(row as f32 * self.line_height()),
        )
    }

    /// The offset nearest `at` (from the text area's top-left, before scrolling): the nearest
    /// glyph boundary on the line under it.
    fn index_at(&self, lines: &[FieldLine], at: Point<Pixels>) -> usize {
        if at.y < px(0.0) {
            return 0;
        }
        let line_height = self.line_height();
        if self.shape == Shape::OneLine && f32::from(at.y) > line_height {
            return self.editing.content.len();
        }
        let row = ((f32::from(at.y) / line_height) as usize).min(lines.len() - 1);
        let line = &lines[row];
        let x = f32::from(at.x) - self.indent(lines);
        let last = self.last_place(lines, row);
        line.glyphs
            .iter()
            .map(|glyph| glyph.start)
            .filter(|&start| start < last)
            .chain([last])
            .min_by(|&one, &other| {
                (line.x_of(one) - x)
                    .abs()
                    .total_cmp(&(line.x_of(other) - x).abs())
            })
            .unwrap_or(line.start)
    }

    /// The last caret position on line `row`: before its newline, or before the spaces it
    /// wrapped at.
    fn last_place(&self, lines: &[FieldLine], row: usize) -> usize {
        let line = &lines[row];
        let words = &self.editing.content[line.start..line.end];
        if words.ends_with('\n') {
            line.end - 1
        } else if row + 1 < lines.len() {
            line.start + words.trim_end_matches(' ').len()
        } else {
            line.end
        }
    }

    /// The offset a line up (`-1`) or down (`1`) from the caret, at the same x.
    fn vertical(&self, lines: i32) -> usize {
        match self.shape {
            Shape::OneLine => {
                if lines < 0 {
                    0
                } else {
                    self.editing.content.len()
                }
            }
            Shape::Wrapped { line_height } => {
                let set = self.lines();
                let at = self.position(&set, self.editing.caret());
                let target = at.y + px(line_height) * (lines as f32 + 0.5);
                if target < px(0.0) {
                    return 0;
                }
                self.index_at(&set, point(at.x, target))
            }
        }
    }

    /// The start or end of the caret's line.
    fn line_end(&self, end: bool) -> usize {
        let set = self.lines();
        let row = Self::line_of(&set, self.editing.caret());
        if end {
            self.last_place(&set, row)
        } else {
            set[row].start
        }
    }

    /// The left offset of a centred single line.
    fn indent(&self, lines: &[FieldLine]) -> f32 {
        match self.align {
            Align::Left => 0.0,
            Align::Centre => ((f32::from(self.words.size.width) - lines[0].width) / 2.0).max(0.0),
        }
    }

    /// Scrolls a single-line field to keep the caret visible.
    fn follow_caret(&mut self, lines: &[FieldLine]) {
        if self.shape != Shape::OneLine {
            return;
        }
        let caret = f32::from(self.position(lines, self.editing.caret()).x);
        let room = f32::from(self.words.size.width);
        let line = lines[0].width;
        if caret - self.scroll > room - SCROLL_MARGIN {
            self.scroll = caret - room + SCROLL_MARGIN;
        } else if caret - self.scroll < 0.0 {
            self.scroll = caret;
        }
        self.scroll = self.scroll.min((line - room).max(0.0)).max(0.0);
    }

    /// The window position of `index`'s top-left.
    fn window_point(&self, lines: &[FieldLine], index: usize) -> Point<Pixels> {
        let at = self.position(lines, index);
        point(
            self.words.origin.x + at.x - px(self.scroll),
            self.words.origin.y + at.y,
        )
    }

    fn local(&self, at: Point<Pixels>) -> Point<Pixels> {
        point(
            at.x - self.words.origin.x + px(self.scroll),
            at.y - self.words.origin.y,
        )
    }

    // MARK: Pointer

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        window.focus(&self.focus, cx);
        self.selecting = true;
        let at = self.index_at(&self.lines(), self.local(event.position));
        if event.click_count >= 3 {
            self.editing.select_all();
        } else if event.click_count == 2 {
            let word = word_at(&self.editing.content, at);
            self.editing.move_to(word.start);
            self.editing.select_to(word.end);
        } else if event.modifiers.shift {
            self.editing.select_to(at);
        } else {
            self.editing.move_to(at);
        }
        self.changed(false, window, cx);
    }

    fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            let at = self.index_at(&self.lines(), self.local(event.position));
            self.select_to(at, window, cx);
        }
    }

    // MARK: Drawing

    pub fn paint(&mut self, window: &mut Window, cx: &mut App) {
        let focused = self.focus.is_focused(window);
        self.paint_ground(window);
        if self.editing.content.is_empty() {
            self.paint_placeholder(window, cx);
        }
        let lines = self.lines();
        self.follow_caret(&lines);
        let room = match self.shape {
            Shape::OneLine => self.words,
            Shape::Wrapped { .. } => self.frame,
        };
        let line_height = self.line_height();
        let (ascent, descent) = text::metrics(self.face);
        let scale = window.scale_factor();
        let selected = self.editing.selected.clone();
        let chosen = (focused && !selected.is_empty()).then_some(selected);
        let ink = strength(color(self.ink), self.enabled);
        window.with_content_mask(Some(ContentMask { bounds: room }), |window| {
            if let Some(selected) = &chosen {
                self.paint_selection(window, &lines, selected);
            }
            let left = f32::from(self.words.origin.x) - self.scroll + self.indent(&lines);
            for (row, line) in lines.iter().enumerate() {
                let top = f32::from(self.words.origin.y) + row as f32 * line_height;
                let baseline = browser_baseline(top, line_height, ascent, descent, scale);
                self.paint_line(window, cx, line, chosen.as_ref(), ink, left, baseline);
            }
            if focused && chosen.is_none() && self.caret_shown {
                let at = self.window_point(&lines, self.editing.caret());
                let caret = Bounds {
                    origin: point(at.x.round(), at.y),
                    size: size(px(1.0), px(line_height)),
                };
                window.paint_quad(gpui::fill(caret, color(self.ink)));
            }
        });
    }

    /// Draws the background: the settings input with its focus border, or the dialog field
    /// with its warning border or focus ring.
    fn paint_ground(&self, window: &mut Window) {
        let focused = self.focus.is_focused(window);
        let frame = self.frame;
        match self.ground {
            Ground::Settings => {
                rounded(window, frame, color(Token::Input), 6.0);
                if focused {
                    stroke(window, inset(frame, 0.5, 0.5), color(Token::Ring), 5.5, 1.0);
                }
            }
            Ground::Dialog { wrong } => {
                rounded(window, frame, color(Token::Field), 6.0);
                if wrong {
                    // An invalid field shows the warning border, focused or not.
                    stroke(
                        window,
                        inset(frame, 0.5, 0.5),
                        color(Token::Destructive),
                        5.5,
                        1.0,
                    );
                } else if focused {
                    soft_ring(window, frame);
                    stroke(window, inset(frame, 0.5, 0.5), color(Token::Ring), 5.5, 1.0);
                }
            }
            Ground::Bare => {}
        }
    }

    fn paint_placeholder(&self, window: &mut Window, cx: &mut App) {
        match &self.placeholder {
            Placeholder::Nothing => {}
            Placeholder::Words(words) => {
                let (x, top) = (
                    f32::from(self.words.origin.x),
                    f32::from(self.words.origin.y),
                );
                crate::text::draw_line(
                    window,
                    cx,
                    words,
                    self.face,
                    strength(color(Token::MutedForeground), self.enabled),
                    x,
                    top,
                    self.line_height(),
                    f32::from(self.words.size.width),
                    None,
                );
            }
            Placeholder::Placed(line) => line.draw(window, cx, Point::default(), None),
        }
    }

    /// Paints one line's glyphs from `left` on `baseline`, with selected text in the accent
    /// foreground and marked text underlined, as AppKit draws a composition.
    #[allow(clippy::too_many_arguments)]
    fn paint_line(
        &self,
        window: &mut Window,
        cx: &mut App,
        line: &FieldLine,
        chosen: Option<&Range<usize>>,
        ink: Hsla,
        left: f32,
        baseline: f32,
    ) {
        let content = &self.editing.content;
        let shown = line.start + content[line.start..line.end].trim_end_matches('\n').len();
        let tinted = |start: usize| chosen.is_some_and(|range| range.contains(&start));
        let mut from = 0;
        while from < line.glyphs.len() {
            let picked = tinted(line.glyphs[from].start);
            let to = (from..line.glyphs.len())
                .find(|&at| tinted(line.glyphs[at].start) != picked)
                .unwrap_or(line.glyphs.len());
            let end = line.glyphs.get(to).map_or(shown, |next| next.start);
            text::paint_glyphs(
                window,
                cx,
                content,
                end,
                &line.glyphs[from..to],
                self.face,
                if picked {
                    color(Token::PrimaryForeground)
                } else {
                    ink
                },
                left,
                baseline,
            );
            from = to;
        }
        if let Some(marked) = &self.editing.marked {
            let (start, end) = (marked.start.max(line.start), marked.end.min(shown));
            if start < end {
                let x = line.x_of(start);
                text::paint_marks(
                    window,
                    self.face,
                    ink,
                    left + x,
                    baseline,
                    line.x_of(end) - x,
                    Some(ink),
                    false,
                );
            }
        }
    }

    fn paint_selection(&self, window: &mut Window, lines: &[FieldLine], selected: &Range<usize>) {
        let line_height = px(self.line_height());
        let ground = color(Token::Primary);
        let (from, to) = (
            self.window_point(lines, selected.start),
            self.window_point(lines, selected.end),
        );
        if from.y == to.y {
            window.paint_quad(gpui::fill(
                Bounds::from_corners(from, point(to.x, to.y + line_height)),
                ground,
            ));
            return;
        }
        // Multi-line: the rest of the first line, all of the middle ones, the start of the last.
        let right = self.words.origin.x + self.words.size.width;
        let left = self.words.origin.x;
        window.paint_quad(gpui::fill(
            Bounds::from_corners(from, point(right, from.y + line_height)),
            ground,
        ));
        if to.y > from.y + line_height {
            window.paint_quad(gpui::fill(
                Bounds::from_corners(point(left, from.y + line_height), point(right, to.y)),
                ground,
            ));
        }
        window.paint_quad(gpui::fill(
            Bounds::from_corners(point(left, to.y), point(to.x, to.y + line_height)),
            ground,
        ));
    }
}

/// One laid-out line: bytes `start..end` (including its newline or wrap spaces), its width,
/// and its glyphs positioned from the line's start.
struct FieldLine {
    start: usize,
    end: usize,
    width: f32,
    glyphs: Vec<Placed>,
}

impl FieldLine {
    /// The x of the caret position before byte `at`, from the line's start: before the first
    /// glyph at or after it (a position inside a glyph's text counts as after it), or after the
    /// last glyph.
    fn x_of(&self, at: usize) -> f32 {
        match self.glyphs.iter().find(|glyph| glyph.start >= at) {
            Some(glyph) => glyph.x as f32,
            None => self
                .glyphs
                .last()
                .map_or(0.0, |glyph| (glyph.x + glyph.advance) as f32),
        }
    }
}

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let content = &self.editing.content;
        let range = range_from_utf16(content, &range);
        actual.replace(range_to_utf16(content, &range));
        Some(content[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: range_to_utf16(&self.editing.content, &self.editing.selected),
            reversed: self.editing.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing.selected = range_from_utf16(&self.editing.content, &range);
        self.editing.reversed = false;
        self.changed(false, window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.editing.content.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.editing
            .marked
            .as_ref()
            .map(|range| range_to_utf16(&self.editing.content, range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing.marked = None;
        self.changed(true, window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range.map(|range| range_from_utf16(&self.editing.content, &range));
        self.edit(range, text, Group::Typing, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.enabled {
            return;
        }
        let range = range.map(|range| range_from_utf16(&self.editing.content, &range));
        self.editing.mark(range, text, selected);
        self.changed(true, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = range_from_utf16(&self.editing.content, &range);
        let lines = self.lines();
        let (from, to) = (
            self.window_point(&lines, range.start),
            self.window_point(&lines, range.end),
        );
        let bottom = from.y.max(to.y) + px(self.line_height());
        Some(Bounds::from_corners(from, point(to.x.max(from.x), bottom)))
    }

    fn selection_bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        self.bounds_for_range(range, bounds, window, cx)
            .into_iter()
            .collect()
    }

    fn character_index_for_point(
        &mut self,
        at: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_at(&self.lines(), self.local(at));
        Some(to_utf16(&self.editing.content, index))
    }
}

impl Focusable for Field {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let focus = self.focus.clone();
        // Expose the text, selection and placeholder as an AppKit text field does.
        let placeholder = match &self.placeholder {
            Placeholder::Nothing => String::new(),
            Placeholder::Words(words) => words.clone(),
            Placeholder::Placed(line) => line.text.clone(),
        };
        let field = super::placed("field", cx.entity_id(), self.frame).role(Role::TextInput);
        let field = if self.label.is_empty() {
            field
        } else {
            field.aria_label(self.label.clone())
        };
        let field = if self.identifier.is_empty() {
            field
        } else {
            #[cfg(target_os = "macos")]
            crate::platform::show_placeholder(&self.identifier, &placeholder);
            field.accessibility_id(self.identifier.clone())
        };
        field
            .aria_value(self.editing.content.clone())
            .aria_placeholder(placeholder)
            .a11y_synthetic_children(super::heard::heard_words(
                self.editing.content.clone(),
                self.editing.selected.clone(),
                self.editing.reversed,
            ))
            .key_context(CONTEXT)
            .track_focus(&focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::top))
            .on_action(cx.listener(Self::bottom))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::paste_and_match_style))
            // As in AppKit: Cut, Copy and Delete need a selection, Undo and Redo need history.
            // The menu bar decides whether Paste is available (`menu_bar`).
            .when(!self.editing.selected.is_empty(), |field| {
                field
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::delete_selection))
            })
            .when(self.editing.can_undo(), |field| {
                field.on_action(cx.listener(Self::undo))
            })
            .when(self.editing.can_redo(), |field| {
                field.on_action(cx.listener(Self::redo))
            })
            .on_action(cx.listener(Self::start_speaking))
            .on_action(cx.listener(Self::stop_speaking))
            .on_action(
                cx.listener(|field, _: &Tab, window, cx| field.command(Command::Tab, window, cx)),
            )
            .on_action(cx.listener(|field, _: &BackTab, window, cx| {
                field.command(Command::BackTab, window, cx)
            }))
            .on_action(cx.listener(|field, _: &Return, window, cx| {
                field.command(Command::Return, window, cx)
            }))
            .on_action(cx.listener(|field, _: &Escape, window, cx| {
                field.command(Command::Escape, window, cx)
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, this.clone()),
                            cx,
                        );
                        this.update(cx, |field, cx| field.paint(window, cx));
                    },
                )
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// Starts the core and installs the text system, which supply theme colours and metrics.
    fn start(cx: &mut TestAppContext) {
        crate::nr::nr_app_start();
        cx.update(|cx| crate::text::install(cx.text_system().clone()));
    }

    #[gpui::test]
    fn one_line_joins_lines_and_wrapped_lines_take_return(cx: &mut TestAppContext) {
        start(cx);
        let window = cx.add_window(|window, cx| Field::new("Name", "", window, cx));
        window
            .update(cx, |field, window, cx| {
                field.replace_text_in_range(None, "one\ntwo", window, cx);
                assert_eq!(
                    field.editing.content, "one two",
                    "a single-line field joins lines with a space"
                );
                field.shape = Shape::Wrapped { line_height: 20.0 };
                field.pass_on(Command::Return, window, cx);
                field.replace_text_in_range(None, "three", window, cx);
                assert_eq!(field.editing.content, "one two\nthree");
            })
            .unwrap();
    }

    /// Undo, Redo, Cut, Copy and Delete are available only when the field can perform them, as
    /// in AppKit.
    #[gpui::test]
    fn edits_are_offered_only_when_they_can_be_made(cx: &mut TestAppContext) {
        start(cx);
        let (field, cx) = cx.add_window_view(|window, cx| Field::new("Name", "", window, cx));
        let offered = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                [
                    window.is_action_available(&Undo, cx),
                    window.is_action_available(&Redo, cx),
                    window.is_action_available(&Cut, cx),
                    window.is_action_available(&Copy, cx),
                    window.is_action_available(&DeleteSelection, cx),
                ]
            })
        };
        cx.update(|window, cx| {
            window.activate_window();
            field.update(cx, |field, cx| field.focus(window, cx));
        });
        assert_eq!(offered(cx), [false; 5], "nothing typed or selected");
        cx.update(|window, cx| {
            field.update(cx, |field, cx| {
                field.replace_text_in_range(None, "tone", window, cx)
            })
        });
        assert_eq!(
            offered(cx),
            [true, false, false, false, false],
            "typed: Undo"
        );
        cx.dispatch_action(SelectAll);
        assert_eq!(
            offered(cx),
            [true, false, true, true, true],
            "selected: Cut, Copy, Delete"
        );
        cx.dispatch_action(Undo);
        assert_eq!(
            offered(cx),
            [false, true, false, false, false],
            "taken back: Redo"
        );
    }

    /// Sending the window behind another keeps the field editing, as AppKit's field editor
    /// does (a rename field stays open). Moving focus within the window ends editing.
    #[gpui::test]
    fn the_window_going_to_the_back_ends_no_editing(cx: &mut TestAppContext) {
        start(cx);
        let (field, cx) =
            cx.add_window_view(|window, cx| Field::new("Name", "tone.wav", window, cx));
        let left = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = left.clone();
        cx.update(|window, cx| {
            cx.subscribe(&field, move |_, _: &Committed, _| {
                counted.set(counted.get() + 1)
            })
            .detach();
            window.activate_window();
            field.update(cx, |field, cx| field.focus(window, cx));
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        assert!(cx.update(|window, _| window.is_window_active()));
        cx.deactivate_window();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(!cx.update(|window, _| window.is_window_active()));
        assert_eq!(
            left.get(),
            0,
            "the window went behind another: still editing"
        );
        assert!(cx.update(|window, cx| field.read(cx).is_focused(window)));
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.blur(cx);
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            left.get(),
            1,
            "the keys left the field: what was typed is the name"
        );
    }

    /// The caret, selection, hit testing and input method rect all align with the core's glyphs.
    #[gpui::test]
    fn every_place_is_where_the_core_set_its_glyph(cx: &mut TestAppContext) {
        start(cx);
        let window = cx.add_window(|window, cx| Field::new("Message", "", window, cx));
        window
            .update(cx, |field, window, cx| {
                field.shape = Shape::Wrapped { line_height: 20.0 };
                field.words = Bounds {
                    origin: point(px(10.0), px(30.0)),
                    size: size(px(60.0), px(100.0)),
                };
                field.replace_text_in_range(None, "one two three four\nfive", window, cx);
                let content = field.editing.content.clone();
                let lines = field.lines();
                let texts: Vec<&str> = lines
                    .iter()
                    .map(|line| &content[line.start..line.end])
                    .collect();
                assert_eq!(texts, ["one two ", "three four\n", "five"]);
                for (row, line) in lines.iter().enumerate() {
                    for glyph in &line.glyphs {
                        let at = field.position(&lines, glyph.start);
                        assert_eq!(f32::from(at.x), glyph.x as f32);
                        assert_eq!(f32::from(at.y), row as f32 * 20.0);
                        let pressed = point(at.x + px(0.1), at.y + px(10.0));
                        assert_eq!(field.index_at(&lines, pressed), glyph.start);
                        let utf16 = to_utf16(&content, glyph.start);
                        let rect = field
                            .bounds_for_range(utf16..utf16, Bounds::default(), window, cx)
                            .expect("a rectangle");
                        assert_eq!(rect.origin, field.window_point(&lines, glyph.start));
                        assert_eq!(rect.origin.x, field.words.origin.x + at.x);
                    }
                }
                // A point past a wrapped line's end maps to before its wrap space.
                let first = &lines[0];
                let far = point(px(500.0), px(10.0));
                assert_eq!(field.index_at(&lines, far), first.end - 1);
                assert_eq!(&content[first.end - 1..first.end], " ");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_field_that_cannot_be_used_takes_no_typing(cx: &mut TestAppContext) {
        start(cx);
        let window = cx.add_window(|window, cx| Field::new("Name", "kept", window, cx));
        window
            .update(cx, |field, window, cx| {
                field.enabled = false;
                field.replace_text_in_range(None, "x", window, cx);
                field.replace_and_mark_text_in_range(None, "に", None, window, cx);
                assert_eq!(field.editing.content, "kept");
            })
            .unwrap();
    }
}
