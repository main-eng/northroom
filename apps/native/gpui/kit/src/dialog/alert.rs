//! A drawn alert box for Linux and Windows (such as the update prompt), where the platform's
//! alert cannot keep Return on the button the core names. It uses the warning dialog's scrim
//! and panel, placed by the core (`menus::ffi::alert_layout`). Return and Escape press their
//! assigned buttons, as `NSAlert` does, and a pressed button answers with its index.

use super::{Dialogs, veil};
use crate::controls::Line;
use crate::controls::{Drawn, Faced, Frame};
use crate::core::{contains, inset};
use crate::motion::{Move, Pose};
use crate::theme::{Token, color, mixed};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, Hsla, MouseButton, MouseDownEvent, MouseUpEvent,
    Pixels, Point, ScrollEvent, Window, px,
};
use northroom_core::dialog::layout::AlertLayout as Layout;
use northroom_core::ffi::{NrKey, NrKeyPress, NrMotion};
use northroom_core::menus::ffi::alert_layout;
use std::rc::Rc;

/// Receives the index of the pressed button.
pub type Answered = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(Clone, Debug)]
pub struct AlertBox {
    pub message: String,
    pub detail: String,
    /// In order. Empty shows the platform's OK.
    pub buttons: Vec<String>,
    /// The button Return presses; the first when `None`.
    pub return_button: Option<usize>,
    /// The button Escape presses.
    pub escape_button: Option<usize>,
    /// Reports an error: the Return button uses the warning colour, like a destructive dialog's
    /// action.
    pub critical: bool,
}

/// The open alert box.
pub(super) struct OpenAlert {
    shown: AlertBox,
    placed: Layout,
    pressed: Option<usize>,
    shade: Move,
    panel: Move,
    answered: Answered,
}

/// The core's layout for `shown` in a view of `size`.
fn layout(shown: &AlertBox, size: gpui::Size<Pixels>) -> Layout {
    let buttons: Vec<&str> = shown.buttons.iter().map(String::as_str).collect();
    alert_layout(
        f64::from(f32::from(size.width)),
        f64::from(f32::from(size.height)),
        &shown.message,
        &shown.detail,
        &buttons,
        returns(shown),
        shown.critical,
    )
}

fn returns(shown: &AlertBox) -> usize {
    shown.return_button.unwrap_or(0)
}

impl OpenAlert {
    fn button_at(&self, at: Point<Pixels>) -> Option<usize> {
        self.placed
            .buttons
            .iter()
            .position(|button| contains(&button.frame.bounds(), at))
    }
}

impl Dialogs {
    /// Shows `shown` until a button is pressed, then calls `answered` with its index.
    pub fn show_alert(
        &mut self,
        shown: AlertBox,
        answered: Answered,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.previous_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        let placed = layout(&shown, window.viewport_size());
        self.alert = Some(OpenAlert {
            shown,
            placed,
            pressed: None,
            shade: Move::entering(NrMotion::Shade),
            panel: Move::entering(NrMotion::Dialog),
            answered,
        });
        cx.notify();
    }

    /// Handles a key while the box is open. Return and Escape press their buttons; other keys
    /// are swallowed.
    pub(super) fn alert_key(
        &mut self,
        press: NrKeyPress,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &self.alert else {
            return;
        };
        let pressed = match press.key {
            NrKey::Return if press.held == 0 => Some(returns(&open.shown)),
            NrKey::Escape if press.held == 0 => open.shown.escape_button,
            _ => None,
        };
        if let Some(place) = pressed {
            self.answer(place, window, cx);
        }
    }

    fn answer(&mut self, place: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.alert.take() else {
            return;
        };
        match self.previous_focus.take() {
            Some(previous) => window.focus(&previous, cx),
            None => window.blur(cx),
        }
        cx.notify();
        let answered = open.answered.clone();
        window.defer(cx, move |window, cx| answered(place, window, cx));
    }

    pub(super) fn alert_down(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.alert {
            open.pressed = open.button_at(at);
            cx.notify();
        }
    }

    pub(super) fn alert_up(
        &mut self,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.alert else {
            return;
        };
        let was = open.pressed.take();
        cx.notify();
        if let Some(place) = was.filter(|&place| open.button_at(at) == Some(place)) {
            self.answer(place, window, cx);
        }
    }

    /// Draws the open box like a destructive dialog.
    pub(super) fn paint_alert(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = window.viewport_size();
        let Some(open) = &mut self.alert else {
            return;
        };
        open.placed = layout(&open.shown, view);
        let placed = &open.placed;
        let whole = Bounds {
            origin: Point::default(),
            size: view,
        };
        window.paint_quad(gpui::fill(
            whole,
            veil(0.5 * open.shade.frame().opacity as f32),
        ));
        let panel = placed.panel.bounds();
        let pose = Pose::of(
            &open.panel.frame(),
            Point::default(),
            Point::default(),
            panel.center(),
        );
        window.paint_quad(gpui::quad(
            pose.rect(panel),
            px(pose.length(14.0)),
            pose.color(color(Token::Background)),
            px(pose.length(1.0)),
            pose.color(color(Token::Border)),
            gpui::BorderStyle::Solid,
        ));
        paint_line(&pose, window, cx, &placed.title);
        for sentence in &placed.message {
            paint_line(&pose, window, cx, sentence);
        }
        for place in 0..placed.buttons.len() {
            open.paint_button(&pose, place, window, cx);
        }
        if open.shade.runs() || open.panel.runs() {
            window.request_animation_frame();
        }
        Self::hold_input_under_alert(window, cx.entity());
    }

    /// Blocks input to everything under the box. Left presses and releases go to its buttons.
    fn hold_input_under_alert(window: &mut Window, dialogs: Entity<Self>) {
        window.on_mouse_event({
            let dialogs = dialogs.clone();
            move |event: &MouseDownEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble {
                    cx.stop_propagation();
                    if event.button == MouseButton::Left {
                        dialogs.update(cx, |dialogs, cx| dialogs.alert_down(event.position, cx));
                    }
                }
            }
        });
        window.on_mouse_event({
            let dialogs = dialogs.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    cx.stop_propagation();
                    dialogs.update(cx, |dialogs, cx| {
                        dialogs.alert_up(event.position, window, cx)
                    });
                }
            }
        });
        window.on_mouse_event(move |_: &ScrollEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                cx.stop_propagation();
            }
        });
    }
}

impl OpenAlert {
    /// Draws button `place` and its label, posed by `pose`.
    fn paint_button(&self, pose: &Pose, place: usize, window: &mut Window, cx: &mut App) {
        let button = &self.placed.buttons[place];
        let frame = button.frame.bounds();
        let pressed = self.pressed == Some(place);
        if place == returns(&self.shown) {
            // The Return button: the accent fill, or the warning colour for a critical box.
            let strength = if pressed { 0.9 } else { 1.0 };
            let fill = if self.shown.critical {
                Token::Destructive
            } else {
                Token::Primary
            };
            paint_rounded(pose, window, frame, color(fill).alpha(strength));
        } else {
            // Other buttons: an outline, like Cancel.
            if pressed {
                paint_rounded(pose, window, frame, mixed(Token::Foreground, 0.12));
            }
            crate::draw::stroke(
                window,
                pose.rect(inset(frame, 0.5, 0.5)),
                pose.color(color(Token::Border)),
                pose.length(5.5),
                pose.length(1.0),
            );
        }
        paint_line(pose, window, cx, &button.words);
    }
}

/// Draws one placed line of the box, posed by `pose`.
fn paint_line(pose: &Pose, window: &mut Window, cx: &mut App, line: &Line) {
    pose.line(
        window,
        cx,
        &line.text,
        line.font.face(),
        line.ink(),
        line.frame.bounds(),
        line.frame.width as f32,
    );
}

/// Fills a button's `frame` with `ink`, its corners rounded, posed by `pose`.
fn paint_rounded(pose: &Pose, window: &mut Window, frame: Bounds<Pixels>, ink: Hsla) {
    window.paint_quad(
        gpui::fill(pose.rect(frame), pose.color(ink)).corner_radii(px(pose.length(6.0))),
    );
}
