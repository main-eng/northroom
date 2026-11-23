//! The button: 26 pt tall, 6 px corners, 12 px medium text, or a dialog button whose text the
//! core placed.
//!
//! `Primary` fills with the accent and `Destructive` with the warning colour, both at 90% on
//! hover. `Outline` has a 1 px border at 10% of the text colour and shades on hover. `Bordered`
//! is a dialog's Cancel: a border-colour outline, shaded on hover and more when pressed. A
//! disabled button draws at half opacity and ignores input. Its dialog draws the focus ring.

use super::Drawn;
use super::{Line, button_ring, flattened, placed, strength};
use crate::core::{frame, inset};
use crate::draw::{rounded, stroke};
use crate::icons::{self, Glyph};
use crate::text::{draw_line, width};
use crate::theme::{Face, Token, color, mixed};
use gpui::{
    App, Bounds, Context, EventEmitter, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Pixels, Render, Rgba, Role, StatefulInteractiveElement, Styled, Window, canvas,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    Primary,
    Outline,
    Destructive,
    Bordered,
}

/// A button's label: its own title, inset 10 pt (or `inset`) after an optional 16 px glyph, or
/// text a dialog layout placed.
#[derive(Clone, Debug, PartialEq)]
pub enum Words {
    Title {
        title: String,
        icon: Option<Glyph>,
        inset: f32,
    },
    Placed(Line),
}

/// The font of a button's own title.
pub fn face() -> Face {
    Face::sans(12.0, 500.0)
}

/// Emitted when a press is released over the button.
pub struct Pressed;

#[derive(Clone)]
pub struct Button {
    pub look: Look,
    pub words: Words,
    pub frame: Bounds<Pixels>,
    pub enabled: bool,
    pub hovered: bool,
    pub pressed: bool,
    /// Draws the focus ring, set while its dialog gives the button focus.
    pub ring: bool,
}

impl EventEmitter<Pressed> for Button {}

impl Button {
    pub fn new(look: Look, words: Words) -> Self {
        Self {
            look,
            words,
            frame: Bounds::default(),
            enabled: true,
            hovered: false,
            pressed: false,
            ring: false,
        }
    }

    /// The size of a self-titled button: text, glyph plus a 6 pt gap, the inset on each side,
    /// and the 1 px border of the filled and outlined looks.
    pub fn size(&self) -> (f32, f32) {
        match &self.words {
            Words::Title { title, icon, inset } => {
                let border = if self.look == Look::Destructive {
                    0.0
                } else {
                    2.0
                };
                let glyph = if icon.is_some() { 16.0 + 6.0 } else { 0.0 };
                (width(title, face()) + glyph + inset * 2.0 + border, 26.0)
            }
            Words::Placed(_) => (
                f32::from(self.frame.size.width),
                f32::from(self.frame.size.height),
            ),
        }
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        cx.notify();
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        let pressed = std::mem::replace(&mut self.pressed, false);
        if pressed && self.enabled {
            cx.emit(Pressed);
        }
        cx.notify();
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Bounds<Pixels>) {
        let words = match self.look {
            Look::Primary | Look::Destructive => self.paint_filled(window, at),
            Look::Outline => self.paint_outline(window, at),
            Look::Bordered => self.paint_bordered(window, at),
        };
        self.paint_words(window, cx, at, words);
        if self.ring {
            button_ring(window, at);
        }
    }

    /// Hovered and enabled.
    fn under_pointer(&self) -> bool {
        self.hovered && self.enabled
    }

    /// Fills `at` with the accent or warning colour (90% on hover). Returns the text colour.
    fn paint_filled(&self, window: &mut Window, at: Bounds<Pixels>) -> Hsla {
        let (token, ink) = if self.look == Look::Primary {
            (Token::Primary, color(Token::PrimaryForeground))
        } else {
            let white: Hsla = Rgba {
                r: 0.99,
                g: 0.99,
                b: 0.99,
                a: 1.0,
            }
            .into();
            (Token::Destructive, white)
        };
        let full = color(token);
        let ground = if self.under_pointer() {
            full.alpha(full.a * 0.9)
        } else {
            full
        };
        rounded(window, at, strength(ground, self.enabled), 6.0);
        if self.enabled {
            ink
        } else {
            strength(flattened(ink, ground), false)
        }
    }

    /// Outlines `at` at 10% of the text colour, shaded on hover. Returns the text colour.
    fn paint_outline(&self, window: &mut Window, at: Bounds<Pixels>) -> Hsla {
        if self.under_pointer() {
            rounded(window, at, mixed(Token::Foreground, 0.08), 6.0);
        }
        let edge = strength(mixed(Token::Foreground, 0.1), self.enabled);
        stroke(window, inset(at, 0.5, 0.5), edge, 5.5, 1.0);
        strength(color(Token::Foreground), self.enabled)
    }

    /// Outlines `at` in the border colour, shaded on hover and more when pressed. Returns the
    /// text colour.
    fn paint_bordered(&self, window: &mut Window, at: Bounds<Pixels>) -> Hsla {
        if self.enabled && (self.pressed || self.hovered) {
            let shade = if self.pressed { 0.12 } else { 0.08 };
            rounded(window, at, mixed(Token::Foreground, shade), 6.0);
        }
        stroke(
            window,
            inset(at, 0.5, 0.5),
            strength(color(Token::Border), self.enabled),
            5.5,
            1.0,
        );
        strength(color(Token::Foreground), self.enabled)
    }

    /// Draws the label in `ink`: the glyph and title, or the placed text.
    fn paint_words(&self, window: &mut Window, cx: &mut App, at: Bounds<Pixels>, ink: Hsla) {
        match &self.words {
            Words::Title { title, icon, inset } => {
                let (x, y) = super::origin_of(at);
                let mut left =
                    x + if self.look == Look::Destructive {
                        0.0
                    } else {
                        1.0
                    } + inset;
                if let Some(icon) = icon {
                    icons::draw(window, cx, *icon, frame(left, y + 5.0, 16.0, 16.0), ink);
                    left += 16.0 + 6.0;
                }
                draw_line(
                    window,
                    cx,
                    title,
                    face(),
                    ink,
                    left,
                    y + 5.0,
                    16.0,
                    f32::from(at.size.width),
                    None,
                );
            }
            Words::Placed(line) => {
                // Keep the layout's text colour, dimmed with the button.
                let ink = strength(line.ink(), self.enabled);
                line.draw(window, cx, gpui::Point::default(), Some(ink));
            }
        }
    }
}

impl Button {
    /// The accessible label.
    fn label(&self) -> String {
        match &self.words {
            Words::Title { title, .. } => title.clone(),
            Words::Placed(line) => line.text.clone(),
        }
    }
}

impl Render for Button {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.clone();
        placed("button", cx.entity_id(), self.frame)
            .role(Role::Button)
            .aria_label(self.label())
            .on_hover(cx.listener(|button, hovered: &bool, _, cx| {
                button.hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|button, _, _, cx| {
                    button.pressed = true;
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|button, _, _, cx| button.release(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|button, _, _, cx| {
                    button.pressed = false;
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    // Paint at the core's frame, not the pixel-rounded layout.
                    move |_, (), window, cx| shown.paint(window, cx, shown.frame),
                )
                .size_full(),
            )
    }
}
