//! Light, Dark, System: three equal-width muted buttons in a row, 36 pt tall. The chosen one is
//! filled with the secondary colour and the hovered one is shaded. Each has 8 pt padding, a
//! 14 px glyph, an 8 pt gap and a 14 px medium label.

use super::placed;
use crate::core::frame;
use crate::draw::rounded;
use crate::icons::{self, Glyph};
use crate::text::{draw_line, width};
use crate::theme::{Face, Token, color, mixed};
use gpui::{
    App, Bounds, Context, EventEmitter, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, StatefulInteractiveElement,
    Styled, Window, canvas,
};

const GLYPHS: [Glyph; 3] = [Glyph::Sun, Glyph::Moon, Glyph::Monitor];
/// Trailing space after the label. It adds to the width and shifts the glyph and label that
/// far left of centre.
const TRAILING: f32 = 4.9;
pub const HEIGHT: f32 = 36.0;

fn face() -> Face {
    Face::sans(14.0, 500.0)
}

/// The index of the picked choice.
pub struct Picked(pub usize);

#[derive(Clone)]
pub struct ThemeChoice {
    pub labels: Vec<String>,
    pub selected: usize,
    pub hovered: Option<usize>,
    pub frame: Bounds<Pixels>,
}

impl EventEmitter<Picked> for ThemeChoice {}

impl ThemeChoice {
    pub fn new(labels: Vec<String>, selected: usize) -> Self {
        Self {
            labels,
            selected,
            hovered: None,
            frame: Bounds::default(),
        }
    }

    /// Each button's width: the mean width the three need.
    pub fn each(&self) -> f32 {
        let total: f32 = self
            .labels
            .iter()
            .map(|label| width(label, face()) + 14.0 + 8.0 + 16.0 + TRAILING)
            .sum();
        total / self.labels.len().max(1) as f32
    }

    pub fn size(&self) -> (f32, f32) {
        (self.each() * self.labels.len() as f32, HEIGHT)
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        cx.notify();
    }

    fn index(&self, at: gpui::Point<Pixels>) -> Option<usize> {
        let x = f32::from(at.x - self.frame.origin.x);
        let index = (x / self.each()).floor();
        (crate::core::contains(&self.frame, at)
            && index >= 0.0
            && (index as usize) < self.labels.len())
        .then_some(index as usize)
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Bounds<Pixels>) {
        let (x, y) = super::origin_of(at);
        let each = self.each();
        let ink = color(Token::Foreground);
        for (index, (label, glyph)) in self.labels.iter().zip(GLYPHS).enumerate() {
            let button = frame(x + index as f32 * each, y, each, HEIGHT);
            if index == self.selected {
                rounded(window, button, color(Token::Secondary), 6.0);
            } else if Some(index) == self.hovered {
                rounded(window, button, mixed(Token::Foreground, 0.08), 6.0);
            }
            let wide = width(label, face());
            let start = x + index as f32 * each + each / 2.0 - (14.0 + 8.0 + wide + TRAILING) / 2.0;
            icons::draw(window, cx, glyph, frame(start, y + 11.0, 14.0, 14.0), ink);
            draw_line(
                window,
                cx,
                label,
                face(),
                ink,
                start + 22.0,
                y + 8.0,
                20.0,
                wide + 1.0,
                None,
            );
        }
    }
}

impl Render for ThemeChoice {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.clone();
        placed("theme", cx.entity_id(), self.frame)
            .on_mouse_move(cx.listener(|choice, event: &MouseMoveEvent, _, cx| {
                let over = choice.index(event.position);
                if over != choice.hovered {
                    choice.hovered = over;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|choice, hovered: &bool, _, cx| {
                if !hovered {
                    choice.hovered = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|choice, event: &MouseDownEvent, _, cx| {
                    if let Some(index) = choice.index(event.position)
                        && index != choice.selected
                    {
                        cx.emit(Picked(index));
                    }
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
