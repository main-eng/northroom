//! The command palette (⌘K). A 50% ink scrim covers the window. The panel has the background
//! colour, a 1 px border and 14 px corners; a search row (magnifier, field, "Esc" key) and a
//! divider sit above the matching commands, grouped under small tracked headings, with the
//! highlighted row shaded and shortcuts at row ends.
//!
//! The core owns the commands, order, layout, highlight and keys (`nr_palette_*`): ↑ and ↓
//! move, Return runs, Escape or a press outside closes, and hover highlights a row.

use crate::controls::input::{self, Changed, Command, Field, Ground, Keys, Placeholder};
use crate::controls::{Drawn, Faced, Frame};
use crate::controls::{Font, Line, Rect};
use crate::core::{c, contains, frame, text, zeroed};
use crate::draw::{fill, rounded, stroke};
use crate::icons::{self, Glyph};
use crate::text::{draw_line, metrics, width};
use crate::theme::{self, Face, Token, color};
use gpui::{
    App, AppContext, Context, DispatchPhase, Entity, EventEmitter, Hitbox, HitboxBehavior,
    IntoElement, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Render, Styled,
    Subscription, Window, canvas, div, prelude::*, size,
};
use northroom_core::commands::{Heading, KeyCap, Parts, PartsRow};
use northroom_core::ffi::{
    NrKey, NrKeyPress, NrPaletteKey, NrText, nr_palette_close, nr_palette_key, nr_palette_open,
    nr_palette_placeholder, nr_palette_point, nr_palette_search, palette_layout,
};

/// What the palette asks of the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Closed; run the command with this id.
    Run(String),
    /// Closed without running anything.
    Closed,
}

/// Text inset from the core's field rect (AppKit's text container padding), and how far typed
/// text and the placeholder sit above the line, matching AppKit's single-line field.
const FIELD_PADDING: f32 = 2.0;
const TYPED_RAISE: f32 = 1.0;
const PLACEHOLDER_RAISE: f32 = 2.25;

pub struct Palette {
    pub field: Entity<Field>,
    /// Shown while the field is empty (`nr_palette_placeholder`).
    placeholder: String,
    parts: Option<Parts>,
    /// The window size at the last layout.
    laid_for: gpui::Size<Pixels>,
    _events: [Subscription; 2],
}

impl EventEmitter<Event> for Palette {}

impl Palette {
    /// Opens the palette (`nr_palette_open`) with every command listed, the first highlighted
    /// and the field focused.
    pub fn open(window: &mut Window, cx: &mut Context<Self>) -> Self {
        nr_palette_open();
        let placeholder = text(&nr_palette_placeholder());
        let field = cx.new(|cx| {
            let mut field = Field::new(&placeholder, "", window, cx);
            field.ground = Ground::Bare;
            field.face = Face::sans(14.0, 400.0);
            field.ink = Token::Foreground;
            field.keys = Keys::Dialog;
            field
        });
        let typed = cx.subscribe(&field, |palette, _, typed: &Changed, cx| {
            let query = c(&typed.0);
            // SAFETY: a NUL-terminated query.
            unsafe { nr_palette_search(query.as_ptr()) };
            palette.parts = None;
            cx.notify();
        });
        let keys = cx.subscribe(&field, |palette, _, command: &Command, cx| match command {
            Command::Return => palette.key(NrKey::Return, cx),
            Command::Escape => palette.key(NrKey::Escape, cx),
            Command::Tab | Command::BackTab => {}
        });
        field.update(cx, |field, cx| field.focus(window, cx));
        Self {
            field,
            placeholder,
            parts: None,
            laid_for: size(Pixels::ZERO, Pixels::ZERO),
            _events: [typed, keys],
        }
    }

    /// The palette's current layout over the window.
    pub fn parts(&self) -> Option<&Parts> {
        self.parts.as_ref()
    }

    fn lay_out(&mut self, view: gpui::Size<Pixels>, cx: &mut Context<Self>) {
        self.parts = palette_layout(f64::from(view.width), f64::from(view.height));
        self.laid_for = view;
        if let Some(parts) = &self.parts {
            let at = parts.field.bounds();
            let (x, y) = (
                f32::from(at.origin.x) + FIELD_PADDING,
                f32::from(at.origin.y),
            );
            let (room, line) = (
                f32::from(at.size.width) - 2.0 * FIELD_PADDING,
                f32::from(at.size.height),
            );
            let words = frame(x, y - TYPED_RAISE, room, line);
            // AppKit sets the placeholder a little higher than what is typed.
            let placeholder = Placeholder::Placed(Line {
                text: self.placeholder.clone(),
                frame: Rect {
                    x: f64::from(x),
                    y: f64::from(y - PLACEHOLDER_RAISE),
                    width: f64::from(room),
                    height: f64::from(line),
                },
                font: Font::sans(14.0, 400.0),
                color: Token::MutedForeground,
                opacity: 1.0,
            });
            self.field.update(cx, |field, cx| {
                field.placeholder = placeholder;
                if field.frame != at || field.words != words {
                    field.place(at, words, cx);
                }
            });
        }
    }

    /// Handles the palette's own keys: ↓, ↑, Return and Escape (`nr_palette_key`).
    fn key(&mut self, key: NrKey, cx: &mut Context<Self>) {
        let mut run: NrText = zeroed();
        // SAFETY: storage for one text.
        match unsafe { nr_palette_key(NrKeyPress::named(key), &mut run) } {
            NrPaletteKey::PassOn => {}
            NrPaletteKey::Redraw => {
                self.parts = None;
                cx.notify();
            }
            NrPaletteKey::Run => {
                let id = text(&run);
                self.close(Event::Run(id), cx);
            }
            NrPaletteKey::Close => self.close(Event::Closed, cx),
        }
    }

    fn close(&mut self, event: Event, cx: &mut Context<Self>) {
        nr_palette_close();
        cx.emit(event);
    }

    fn mouse_down(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(parts) = &self.parts else { return };
        // A press outside the panel closes it.
        if !contains(&parts.panel.bounds(), at) {
            return self.close(Event::Closed, cx);
        }
        if let Some(row) = parts
            .rows
            .iter()
            .find(|row| contains(&row.frame.bounds(), at))
        {
            let id = row.id.to_owned();
            self.close(Event::Run(id), cx);
        }
    }

    fn mouse_move(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(parts) = &self.parts else { return };
        if let Some(index) = parts
            .rows
            .iter()
            .position(|row| contains(&row.frame.bounds(), at))
            && nr_palette_point(index)
        {
            self.parts = None;
            cx.notify();
        }
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let view = window.viewport_size();
        if self.parts.is_none() || self.laid_for != view {
            self.lay_out(view, cx);
        }
        let Some(parts) = self.parts.clone() else {
            return;
        };
        fill(
            window,
            frame(0.0, 0.0, f32::from(view.width), f32::from(view.height)),
            color(Token::Ink).alpha(0.5),
        );
        let panel = parts.panel.bounds();
        // Fill the path the border strokes, so the border's outer half lies on the scrim.
        rounded(
            window,
            crate::core::inset(panel, 0.5, 0.5),
            color(Token::Background),
            13.5,
        );
        stroke(
            window,
            crate::core::inset(panel, 0.5, 0.5),
            color(Token::Border),
            13.5,
            1.0,
        );
        icons::draw(
            window,
            cx,
            Glyph::MagnifyingGlass,
            parts.magnifier.bounds(),
            color(Token::MutedForeground),
        );
        key_cap(window, cx, &parts.escape);
        fill(window, parts.divider.bounds(), color(Token::Border));
        window.with_content_mask(
            Some(gpui::ContentMask {
                bounds: parts.clip.bounds(),
            }),
            |window| {
                if let Some(empty) = &parts.empty {
                    empty.draw(window, cx, Point::default(), None);
                }
                for row in &parts.rows {
                    paint_row(window, cx, row);
                }
            },
        );
        self.listen(hitbox, window, cx);
    }

    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let palette = cx.entity();
        let over = hitbox.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                palette.update(cx, |palette, cx| palette.mouse_down(event.position, cx));
            }
        });
        let palette = cx.entity();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                palette.update(cx, |palette, cx| palette.mouse_move(event.position, cx));
            }
        });
    }
}

/// Draws a palette row: its heading, highlight, shortcut key and title.
fn paint_row(window: &mut Window, cx: &mut App, row: &PartsRow) {
    if let Some(heading) = &row.heading {
        paint_heading(window, cx, heading);
    }
    if row.lit {
        rounded(
            window,
            row.frame.bounds(),
            color(Token::Foreground).alpha(0.08),
            2.0,
        );
    }
    if let Some(shortcut) = &row.key {
        key_cap(window, cx, shortcut);
    }
    row.title.draw(window, cx, Point::default(), None);
}

/// Draws a key cap: a small fill of the text colour at 10% with the key's name.
fn key_cap(window: &mut Window, cx: &mut App, key: &KeyCap) {
    fill(
        window,
        key.frame.bounds(),
        color(Token::Foreground).alpha(0.1),
    );
    key.words.draw(window, cx, Point::default(), None);
}

/// Draws a heading as `NSAttributedString.draw(at:)` does: line top at `y`, with `tracking`
/// points after each letter.
fn paint_heading(window: &mut Window, cx: &mut App, heading: &Heading) {
    let face = heading.font.face();
    let ink = theme::faded(theme::color(heading.color), heading.opacity);
    let (ascent, descent) = metrics(face);
    let mut x = heading.x as f32;
    let mut letter = [0u8; 4];
    for character in heading.text.chars() {
        let letter = character.encode_utf8(&mut letter);
        draw_line(
            window,
            cx,
            letter,
            face,
            ink,
            x,
            heading.y as f32,
            ascent + descent,
            f32::MAX,
            None,
        );
        x += width(letter, face) + heading.tracking as f32;
    }
}

impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = cx.entity();
        div()
            .id(("kit-palette", cx.entity_id()))
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            // The palette takes every key the field does not, including ↑ and ↓.
            .capture_action(cx.listener(|palette, _: &input::Up, _, cx| {
                palette.key(NrKey::Up, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|palette, _: &input::Down, _, cx| {
                palette.key(NrKey::Down, cx);
                cx.stop_propagation();
            }))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::BlockMouse),
                    move |_, hitbox, window, cx| {
                        palette.update(cx, |palette, cx| palette.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
            .child(self.field.clone())
    }
}
