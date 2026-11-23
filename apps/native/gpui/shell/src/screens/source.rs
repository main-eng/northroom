//! A source file that is not a page, table or plain text, shown as is. Monospace 12 px on
//! 16 px lines, inset 16 px, on the panel background. Lines don't wrap, the view scrolls both
//! ways, and the text can be selected and copied but not edited.
//!
//! Syntax colouring comes from the core's highlighter (`webhost::syntax`), always in the dark
//! theme's colours, and the text stays plain until the grammars compile. The core shapes each
//! line and `kit::text` draws it; hit testing and selection use the same shaped lines.

use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use gpui::{
    App, Bounds, Context, CursorStyle, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollDelta, ScrollEvent, Styled,
    Task, Window, canvas, point, px,
};
use kit::controls::placed;
use kit::core::{c, contains, frame, text};
use kit::draw::fill;
use kit::lists::scroll::Scroller;
use kit::motion::now;
use kit::nr::*;
use kit::platform::write_clipboard;
use kit::text::{Run, SetLine, Setting, draw_set_lines, set};
use kit::theme::{Face, Token, color, mixed};
use std::cell::RefCell;
use std::rc::Rc;

/// Monospace 12 px on 16 px lines, inset 16 px.
const SIZE: f32 = 12.0;
const LINE: f32 = 16.0;
const INSET: f32 = 16.0;

fn face() -> Face {
    Face::mono(SIZE)
}

/// A run of one line in one colour and style; `color` of `None` is the theme foreground.
struct Piece {
    text: String,
    color: Option<Hsla>,
    bold: bool,
    underline: bool,
    strike: bool,
}

/// A source line: its text with tabs expanded, and its pieces.
#[derive(Default)]
struct Line {
    text: String,
    pieces: Vec<Piece>,
    /// The last shaped line and the foreground it used. A line's text never changes (new text
    /// makes new lines), so it is shaped again only when the theme foreground changes.
    setting: RefCell<Option<(Hsla, Rc<Set>)>>,
}

/// A line's runs and the shaped line; `None` when the line is empty.
type Set = (Vec<Run>, Option<SetLine>);

impl Line {
    /// The pieces as kit runs, each in its colour (the theme foreground if it has none) and
    /// style.
    fn runs(&self, foreground: Hsla) -> Vec<Run> {
        self.pieces
            .iter()
            .map(|piece| {
                let ink = piece.color.unwrap_or(foreground);
                Run {
                    text: piece.text.clone(),
                    face: face_of(piece),
                    color: ink,
                    underline: piece.underline.then_some(ink),
                    strike: piece.strike,
                    code: false,
                    space: 0.0,
                    tracking: 0.0,
                }
            })
            .collect()
    }

    /// The line shaped by the core on one line; `None` when empty. Cached per foreground.
    fn set(&self, foreground: Hsla) -> Rc<Set> {
        let mut setting = self.setting.borrow_mut();
        if let Some((ink, kept)) = setting.as_ref()
            && *ink == foreground
        {
            return Rc::clone(kept);
        }
        let runs = self.runs(foreground);
        let line = set(&runs, f32::INFINITY, false).into_iter().next();
        let kept = Rc::new((runs, line));
        *setting = Some((foreground, Rc::clone(&kept)));
        kept
    }
}

/// The caret stops on a shaped line: each byte offset where a glyph starts, plus the end, with
/// its x offset from the line start.
fn places(line: &Line, set: Option<&SetLine>) -> Vec<(usize, f32)> {
    let Some(set) = set else {
        return vec![(0, 0.0)];
    };
    let mut starts = Vec::with_capacity(line.pieces.len());
    let mut at = 0;
    for piece in &line.pieces {
        starts.push(at);
        at += piece.text.len();
    }
    set.pieces
        .iter()
        .flat_map(|piece| {
            piece
                .glyphs
                .iter()
                .map(|glyph| (starts[piece.run] + glyph.start, piece.x + glyph.x as f32))
        })
        .chain([(line.text.len(), set.width)])
        .collect()
}

/// The x of the caret before byte `at`: before the first glyph at or after it.
fn x_of(places: &[(usize, f32)], at: usize) -> f32 {
    places
        .iter()
        .find(|(start, _)| *start >= at)
        .map_or(0.0, |(_, x)| *x)
}

/// Expands tabs in `part` to spaces, with stops every `tab` columns. `part` starts `column`
/// characters into its line.
pub(super) fn expand_tabs(part: &str, column: usize, tab: usize) -> String {
    let mut set = String::with_capacity(part.len());
    for character in part.chars() {
        if character == '\t' {
            let column = column + set.chars().count();
            set.extend(std::iter::repeat_n(' ', tab - column % tab));
        } else {
            set.push(character);
        }
    }
    set
}

/// The x span the selection `start..end` covers on line `index` (shaped as `set`), if any. A
/// line the selection runs past is covered one space beyond its end.
fn selected_span(
    line: &Line,
    index: usize,
    set: Option<&SetLine>,
    (start, end): (Spot, Spot),
    space: f32,
) -> Option<(f32, f32)> {
    if start == end || !(start.line..=end.line).contains(&index) {
        return None;
    }
    let places = places(line, set);
    let from = if index == start.line {
        x_of(&places, start.at)
    } else {
        0.0
    };
    let to = if index == end.line {
        x_of(&places, end.at)
    } else {
        x_of(&places, line.text.len()) + space
    };
    Some((from, to))
}

/// A position in the text: a line and a byte offset in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
struct Spot {
    line: usize,
    at: usize,
}

pub struct Source {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The text as the core last gave it.
    shown: Option<String>,
    lines: Vec<Line>,
    /// The width of the widest shaped line.
    widest: f32,
    down: Scroller,
    across: Scroller,
    /// Where the selection started and where it ends now.
    anchor: Spot,
    head: Spot,
    selecting: bool,
    /// The pending colouring for the shown text.
    _colouring: Option<Task<()>>,
}

impl Source {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut source = Self {
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            shown: None,
            lines: Vec::new(),
            widest: 0.0,
            down: Scroller::default(),
            across: Scroller::default(),
            anchor: Spot::default(),
            head: Spot::default(),
            selecting: false,
            _colouring: None,
        };
        source.reload(window, cx);
        source
    }

    /// Shows `pieces` (the whole text) split into lines, with the core's tab width.
    fn show(&mut self, pieces: Vec<Piece>) {
        let tab = nr_source_tab_spaces() as usize;
        let mut lines = vec![Line::default()];
        for piece in pieces {
            for (index, part) in piece.text.split('\n').enumerate() {
                if index > 0 {
                    lines.push(Line::default());
                }
                if part.is_empty() {
                    continue;
                }
                let line = lines.last_mut().expect("one line at least");
                let set = expand_tabs(part, line.text.chars().count(), tab);
                line.text.push_str(&set);
                line.pieces.push(Piece { text: set, ..piece });
            }
        }
        let advance = kit::text::width(" ", face());
        self.widest = lines
            .iter()
            .map(|line| line.text.chars().count() as f32 * advance)
            .fold(0.0, f32::max);
        self.lines = lines;
        let end = self.end();
        self.anchor = self.anchor.min(end);
        self.head = self.head.min(end);
    }

    fn plain(text: &str) -> Vec<Piece> {
        vec![Piece {
            text: text.to_owned(),
            color: None,
            bold: false,
            underline: false,
            strike: false,
        }]
    }

    /// The core's coloured runs of its last answer (`nr_syntax_run`).
    fn coloured(count: usize) -> Vec<Piece> {
        let mut pieces = Vec::with_capacity(count);
        for index in 0..count {
            let mut run: NrSyntaxRun = kit::core::zeroed();
            // SAFETY: storage for one run.
            if !unsafe { nr_syntax_run(index, &mut run) } {
                continue;
            }
            pieces.push(Piece {
                text: text(&run.text),
                color: run.colored.then(|| {
                    kit::display::shown(Rgba {
                        r: run.color.red,
                        g: run.color.green,
                        b: run.color.blue,
                        a: run.color.alpha,
                    })
                }),
                bold: run.bold,
                underline: run.underline,
                strike: run.strikethrough,
            });
        }
        pieces
    }

    /// Requests syntax colours; they replace the plain text once the core has them.
    fn colour(&mut self, shown: String, cx: &mut Context<Self>) {
        let call = c(&shown);
        // SAFETY: a NUL-terminated text.
        let call = text(&unsafe { nr_opened_syntax_call(call.as_ptr()) });
        if call.is_empty() {
            self._colouring = None;
            return;
        }
        let answer = webhost::syntax::settle(&call);
        self._colouring = Some(cx.spawn(async move |source, cx| {
            let answer = answer.await;
            let _ = source.update(cx, |source, cx| {
                if source.shown.as_deref() != Some(shown.as_str()) {
                    return;
                }
                let answer = c(&answer);
                // SAFETY: a NUL-terminated answer.
                let count = unsafe { nr_syntax_answer(answer.as_ptr()) };
                if count > 0 {
                    source.show(Self::coloured(count));
                    cx.notify();
                }
            });
        }));
    }

    fn end(&self) -> Spot {
        let line = self.lines.len().saturating_sub(1);
        Spot {
            line,
            at: self.lines.get(line).map_or(0, |line| line.text.len()),
        }
    }

    /// The text origin in the window at the current scroll.
    fn origin(&self) -> Point<Pixels> {
        point(
            self.frame.origin.x + px(INSET - self.across.offset()),
            self.frame.origin.y + px(INSET - self.down.offset()),
        )
    }

    /// The text position nearest `at`.
    fn spot_at(&self, at: Point<Pixels>) -> Spot {
        let origin = self.origin();
        let row = (f32::from(at.y - origin.y) / LINE).floor();
        if row < 0.0 {
            return Spot::default();
        }
        if row as usize >= self.lines.len() {
            return self.end();
        }
        let line = row as usize;
        let kept = self.lines[line].set(color(Token::Foreground));
        let x = f32::from(at.x - origin.x);
        let at = places(&self.lines[line], kept.1.as_ref())
            .into_iter()
            .min_by(|one, other| (one.1 - x).abs().total_cmp(&(other.1 - x).abs()))
            .map_or(0, |(at, _)| at);
        Spot { line, at }
    }

    fn ordered(&self) -> (Spot, Spot) {
        (self.anchor.min(self.head), self.anchor.max(self.head))
    }

    fn selected_text(&self) -> String {
        let (start, end) = self.ordered();
        let mut out = String::new();
        for index in start.line..=end.line.min(self.lines.len().saturating_sub(1)) {
            let words = &self.lines[index].text;
            let from = if index == start.line { start.at } else { 0 };
            let to = if index == end.line {
                end.at
            } else {
                words.len()
            };
            out.push_str(&words[from.min(to)..to]);
            if index < end.line {
                out.push('\n');
            }
        }
        out
    }

    /// Selects the word or line around `spot`, for a double or triple click.
    fn select_around(&mut self, spot: Spot, clicks: usize) {
        let words = &self.lines[spot.line].text;
        if clicks >= 3 {
            self.anchor = Spot {
                line: spot.line,
                at: 0,
            };
            self.head = Spot {
                line: spot.line,
                at: words.len(),
            };
            return;
        }
        let word = |character: char| character.is_alphanumeric() || character == '_';
        let start = words[..spot.at]
            .char_indices()
            .rev()
            .take_while(|(_, character)| word(*character))
            .last()
            .map_or(spot.at, |(index, _)| index);
        let end = words[spot.at..]
            .char_indices()
            .find(|(_, character)| !word(*character))
            .map_or(words.len(), |(index, _)| spot.at + index);
        self.anchor = Spot {
            line: spot.line,
            at: start,
        };
        self.head = Spot {
            line: spot.line,
            at: end,
        };
    }

    // MARK: Pointer and keys

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        if self.down.press(event.position, self.frame, now) {
            cx.notify();
            return;
        }
        window.focus(&self.focus, cx);
        if event.button != MouseButton::Left {
            return;
        }
        let spot = self.spot_at(event.position);
        if event.click_count > 1 {
            self.select_around(spot, event.click_count);
        } else {
            if !event.modifiers.shift {
                self.anchor = spot;
            }
            self.head = spot;
        }
        self.selecting = true;
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, over: bool, cx: &mut Context<Self>) {
        let now = now();
        if self.down.is_held() {
            self.down.pointer(event.position, self.frame, now);
            cx.notify();
            return;
        }
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            self.head = self.spot_at(event.position);
            cx.notify();
            return;
        }
        if over && self.down.pointer(event.position, self.frame, now) {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        if self.down.release(now()) {
            cx.notify();
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let now = now();
        let mut moved = self.down.wheel(event, now);
        // Horizontal travel goes to the horizontal scroller as its own scroll axis.
        let across = ScrollEvent {
            delta: match event.delta {
                ScrollDelta::Lines(delta) => ScrollDelta::Lines(point(0.0, delta.x)),
                ScrollDelta::Pixels(delta) => ScrollDelta::Pixels(point(px(0.0), delta.x)),
            },
            ..event.clone()
        };
        moved |= self.across.wheel(&across, now);
        if moved {
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keys = &event.keystroke;
        if !keys.modifiers.platform {
            return;
        }
        match keys.key.as_str() {
            "a" => {
                self.anchor = Spot::default();
                self.head = self.end();
                cx.notify();
            }
            "c" if self.anchor != self.head => {
                write_clipboard(&self.selected_text(), cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        let view = self.frame;
        fill(window, view, color(Token::Panel));
        self.down.set_extent(
            f32::from(view.size.height),
            self.lines.len() as f32 * LINE + 2.0 * INSET,
        );
        self.across
            .set_extent(f32::from(view.size.width), self.widest + 2.0 * INSET);
        window.with_content_mask(Some(gpui::ContentMask { bounds: view }), |window| {
            self.paint_lines(window, cx);
            self.down.paint_bar(window, view, now);
            self.across.paint_bar_across(window, view, now);
        });
        if self.down.tick(now) | self.across.tick(now) {
            window.request_animation_frame();
        }
        window.set_cursor_style(CursorStyle::IBeam, hitbox);
        self.listen(hitbox, window, cx);
    }

    fn paint_lines(&self, window: &mut Window, cx: &mut App) {
        let origin = self.origin();
        let (top, bottom) = (
            self.frame.origin.y,
            self.frame.origin.y + self.frame.size.height,
        );
        let first = (f32::from(top - origin.y) / LINE).floor().max(0.0) as usize;
        let last =
            ((f32::from(bottom - origin.y) / LINE).ceil().max(0.0) as usize).min(self.lines.len());
        let foreground = color(Token::Foreground);
        let selection = mixed(Token::Primary, 0.25);
        let (start, end) = self.ordered();
        let space = kit::text::width(" ", face());
        let left = f32::from(origin.x);
        for index in first..last {
            let line = &self.lines[index];
            let line_top = f32::from(origin.y) + index as f32 * LINE;
            let kept = line.set(foreground);
            let (runs, set) = &*kept;
            if let Some((from, to)) = selected_span(line, index, set.as_ref(), (start, end), space)
            {
                fill(
                    window,
                    frame(left + from, line_top, to - from, LINE),
                    selection,
                );
            }
            if let Some(set) = set {
                draw_set_lines(
                    window,
                    cx,
                    runs,
                    std::slice::from_ref(set),
                    left,
                    line_top,
                    LINE,
                    face(),
                    Setting::Browser,
                    None,
                );
            }
        }
    }

    /// Registers pointer handlers while the source is drawn: over the view, and anywhere in the
    /// window while one of its presses is held.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let source = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            source.update(cx, |source, cx| source.mouse_down(event, window, cx));
        });
        let (source, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
            let hovered = over.is_hovered(window);
            source.update(cx, |source, cx| source.mouse_move(event, hovered, cx));
        });
        let source = entity.clone();
        on_bubble(window, move |event: &MouseUpEvent, _, cx| {
            if event.button == MouseButton::Left {
                source.update(cx, |source, cx| source.mouse_up(cx));
            }
        });
        let source = entity;
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            source.update(cx, |source, cx| source.scroll(event, cx));
        });
    }
}

/// The monospace face for a piece's style. Bold is Geist Mono 700, matching AppKit's bold trait.
/// Geist Mono has no italic, so italic pieces stay upright, as AppKit leaves them.
fn face_of(piece: &Piece) -> Face {
    if piece.bold {
        Face {
            weight: 700.0,
            ..face()
        }
    } else {
        face()
    }
}

impl EventEmitter<Ask> for Source {}

impl Screen for Source {
    /// Reads the open file from the core again. Changed text replaces what is shown, coloured if
    /// the core already has colours and plain until it does.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let now = text(&nr_opened_text());
        if self.shown.as_deref() == Some(now.as_str()) {
            return;
        }
        self.shown = Some(now.clone());
        self.show(Self::plain(&now));
        self.anchor = Spot::default();
        self.head = Spot::default();
        self.down = Scroller::default();
        self.across = Scroller::default();
        self.colour(now, cx);
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "SourceFileView".to_owned();
        }
    }

    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        contains(&self.frame, at).then(|| {
            (
                self.down.offset(),
                f32::from(self.frame.size.height),
                self.lines.len() as f32 * LINE + 2.0 * INSET,
            )
        })
    }
}

impl Render for Source {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let source = cx.entity();
        placed("source", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        source.update(cx, |source, cx| source.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
    }
}
