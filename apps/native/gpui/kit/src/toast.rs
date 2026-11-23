//! A short notice at the window's lower right: what went wrong, with the reason in quieter text
//! below. The core sets its layout (`menus::ffi::toast_layout`), how long it stays and how many
//! show at once (`nr_toast_policy`). Pressing it dismisses it.

use crate::controls::{Drawn, Frame};
use crate::core::contains;
use crate::theme::{Token, color};
use gpui::{
    App, Bounds, Context, DispatchPhase, HitboxBehavior, IntoElement, MouseDownEvent, Pixels,
    Render, Task, Window, canvas, div, prelude::*, px,
};
use northroom_core::dialog::layout::ToastLayout as Layout;
use northroom_core::menus::ffi::{nr_toast_policy, toast_layout};
use std::time::Duration;

struct Toast {
    serial: u64,
    placed: Layout,
    _goes: Task<()>,
}

impl Toast {
    fn frame(&self) -> Bounds<Pixels> {
        self.placed.frame.bounds()
    }
}

/// The notices showing, the oldest first.
pub struct Toasts {
    shown: Vec<Toast>,
    made: u64,
}

impl Toasts {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            shown: Vec::new(),
            made: 0,
        }
    }

    /// Shows `title` with the reason in `detail`. It dismisses itself after the core's timeout,
    /// and the oldest notices give way per the core's limit.
    pub fn show(&mut self, title: &str, detail: &str, window: &mut Window, cx: &mut Context<Self>) {
        let policy = nr_toast_policy();
        let keep = policy.at_once.saturating_sub(1);
        if self.shown.len() > keep {
            self.shown.drain(..self.shown.len() - keep);
        }
        let view = window.viewport_size();
        let placed = toast_layout(title, detail, f64::from(view.width), f64::from(view.height));
        self.made += 1;
        let serial = self.made;
        let seconds = Duration::from_secs_f64(policy.seconds);
        let goes = cx.spawn(async move |toasts, cx| {
            cx.background_executor().timer(seconds).await;
            let _ = toasts.update(cx, |toasts, cx| toasts.take_away(serial, cx));
        });
        self.shown.push(Toast {
            serial,
            placed,
            _goes: goes,
        });
        cx.notify();
    }

    fn take_away(&mut self, serial: u64, cx: &mut Context<Self>) {
        self.shown.retain(|toast| toast.serial != serial);
        cx.notify();
    }

    /// The titles of the notices showing, for tests.
    pub fn titles(&self) -> Vec<String> {
        self.shown
            .iter()
            .map(|toast| toast.placed.title.text.clone())
            .collect()
    }

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for toast in &self.shown {
            draw(toast, window, cx);
        }
        let toasts = cx.entity();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let hit = toasts
                .read(cx)
                .shown
                .iter()
                .rev()
                .find(|toast| contains(&toast.frame(), event.position))
                .map(|toast| toast.serial);
            if let Some(serial) = hit {
                cx.stop_propagation();
                toasts.update(cx, |toasts, cx| toasts.take_away(serial, cx));
            }
        });
    }
}

/// Draws the notice on the popover surface with a 1 px border and 10 px corners.
fn draw(toast: &Toast, window: &mut Window, cx: &mut App) {
    let frame = toast.frame();
    window.paint_quad(gpui::quad(
        frame,
        px(10.0),
        color(Token::Popover),
        px(1.0),
        color(Token::Border),
        gpui::BorderStyle::Solid,
    ));
    toast.placed.title.draw(window, cx, frame.origin, None);
    for line in &toast.placed.detail {
        line.draw(window, cx, frame.origin, None);
    }
}

impl Render for Toasts {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.shown.is_empty() {
            return div().into_any_element();
        }
        let view = window.viewport_size();
        let frames: Vec<Bounds<Pixels>> = self.shown.iter().map(Toast::frame).collect();
        let this = cx.entity();
        crate::popover::layer(
            canvas(
                move |_, window, _| {
                    for frame in &frames {
                        window.insert_hitbox(*frame, HitboxBehavior::BlockMouse);
                    }
                },
                move |_, (), window, cx| this.update(cx, |toasts, cx| toasts.paint(window, cx)),
            )
            .w(view.width)
            .h(view.height),
            crate::popover::Layer::Toast,
        )
    }
}
