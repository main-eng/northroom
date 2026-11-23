//! The select: a muted filled button, 26 pt tall with 6 px corners, showing the current choice
//! and a caret. A choice may show a round 12 px outlined swatch before its name (the accent
//! choices). A press emits `Opened`, and the owner shows `menu()` with `menu::Menus::open_below`
//! 4 pt below the select. A disabled select draws at half opacity and opens nothing. Its dialog
//! draws the focus ring.

use super::{placed, soft_ring, strength};
use crate::core::{frame, inset};
use crate::draw::{oval, rounded, stroke};
use crate::icons::{self, Glyph};
use crate::menu_model::{Check, Entry, Menu};
use crate::text::{draw_line, width};
use crate::theme::{Face, Token, color, faded};
use gpui::{
    App, Bounds, ContentMask, Context, EventEmitter, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Render, StatefulInteractiveElement, Styled, Window, canvas,
};

pub const HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swatch {
    pub color: Hsla,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: String,
    pub name: String,
    pub swatch: Option<Swatch>,
}

fn face() -> Face {
    Face::sans(12.0, 400.0)
}

/// A press opened the select. The owner shows its choices 4 pt below `frame`, the current one
/// ticked, at least as wide as the select.
pub struct Opened {
    pub frame: Bounds<Pixels>,
}

#[derive(Clone)]
pub struct Select {
    pub choices: Vec<Choice>,
    pub selected: usize,
    pub frame: Bounds<Pixels>,
    pub enabled: bool,
    pub hovered: bool,
    /// Draws the focus ring, set while its dialog gives the select focus.
    pub ring: bool,
}

impl EventEmitter<Opened> for Select {}

impl Select {
    pub fn new(choices: Vec<Choice>, selected: usize) -> Self {
        Self {
            choices,
            selected,
            frame: Bounds::default(),
            enabled: true,
            hovered: false,
            ring: false,
        }
    }

    /// The select's width when fitted to the current choice (`w-fit`): 8 pt padding, the swatch
    /// plus 8 if any, the name, 8, the 14 px caret, 8.
    pub fn fitted_width(&self) -> f32 {
        let Some(choice) = self.choices.get(self.selected) else {
            return 8.0 + 8.0 + 14.0 + 8.0;
        };
        let swatch = choice.swatch.map_or(0.0, |_| 12.0 + 8.0);
        8.0 + swatch + width(&choice.name, face()) + 8.0 + 14.0 + 8.0
    }

    /// The choices as a menu, the current one ticked, at least as wide as the select. Entry ids
    /// are the choice ids. Swatches are left out, since menu entries hold glyphs, not colours.
    pub fn menu(&self) -> Menu {
        let entries = self
            .choices
            .iter()
            .enumerate()
            .map(|(index, choice)| Entry {
                check: if index == self.selected {
                    Check::On
                } else {
                    Check::Off
                },
                ..Entry::item(choice.id.clone(), choice.name.clone())
            })
            .collect();
        Menu {
            min_width: f64::from(f32::from(self.frame.size.width)),
            ..Menu::new("", entries)
        }
    }

    /// The menu's top-left: 4 pt below the select.
    pub fn menu_origin(&self) -> gpui::Point<Pixels> {
        gpui::point(
            self.frame.origin.x,
            self.frame.origin.y + self.frame.size.height + gpui::px(4.0),
        )
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        cx.notify();
    }

    /// Opens the menu as a press does (Return or Space in a dialog).
    pub fn open(&mut self, cx: &mut Context<Self>) {
        if self.enabled {
            cx.emit(Opened { frame: self.frame });
        }
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Bounds<Pixels>) {
        let (x, y) = super::origin_of(at);
        let (w, h) = (f32::from(at.size.width), f32::from(at.size.height));
        let shade = |color: Hsla| strength(color, self.enabled);
        let ground = color(Token::Field);
        let ground = if self.hovered && self.enabled {
            ground.alpha(ground.a * 0.8)
        } else {
            ground
        };
        rounded(window, at, shade(ground), 6.0);
        if let Some(choice) = self.choices.get(self.selected) {
            let mut left = x + 8.0;
            if let Some(swatch) = choice.swatch {
                let dot = frame(left, y + 7.0, 12.0, 12.0);
                oval(window, dot, shade(swatch.color));
                stroke(
                    window,
                    inset(dot, 0.5, 0.5),
                    shade(color(Token::Border)),
                    5.5,
                    1.0,
                );
                left += 12.0 + 8.0;
            }
            // Clip a name that does not fit, with no ellipsis.
            let room = frame(left, y, x + w - left - 8.0 - 14.0 - 8.0, h);
            window.with_content_mask(Some(ContentMask { bounds: room }), |window| {
                draw_line(
                    window,
                    cx,
                    &choice.name,
                    face(),
                    shade(color(Token::FieldForeground)),
                    left,
                    y + 5.0,
                    16.0,
                    1000.0,
                    None,
                );
            });
        }
        icons::draw(
            window,
            cx,
            Glyph::CaretDown,
            frame(x + w - 8.0 - 14.0, y + 6.0, 14.0, 14.0),
            shade(faded(color(Token::MutedForeground), 0.5)),
        );
        if self.ring {
            soft_ring(window, at);
        }
    }
}

impl Render for Select {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.clone();
        placed("select", cx.entity_id(), self.frame)
            .on_hover(cx.listener(|select, hovered: &bool, _, cx| {
                select.hovered = *hovered && select.enabled;
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|select, _, _, cx| select.open(cx)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_menu_ticks_the_chosen_choice_and_is_as_wide_as_the_select() {
        let choice = |id: &str| Choice {
            id: id.to_owned(),
            name: id.to_uppercase(),
            swatch: None,
        };
        let mut select = Select::new(vec![choice("low"), choice("high")], 1);
        select.frame = crate::core::frame(10.0, 20.0, 140.0, HEIGHT);
        let menu = select.menu();
        let ticks: Vec<(String, Check)> = menu
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), entry.check))
            .collect();
        assert_eq!(
            ticks,
            [
                ("low".to_owned(), Check::Off),
                ("high".to_owned(), Check::On)
            ]
        );
        assert_eq!(menu.min_width, 140.0);
        assert_eq!(
            select.menu_origin(),
            gpui::point(gpui::px(10.0), gpui::px(50.0))
        );
    }
}
