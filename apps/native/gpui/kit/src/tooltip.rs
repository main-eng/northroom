//! Tooltips: popover colour, 12 px type, 6 px corners and a small diamond pointing at the
//! control. The core places it under the control (`nr_tooltip_place`) and decides when it shows
//! (`nr_tooltip_rest`, `nr_tooltip_due`, `nr_tooltip_leave`), so the delay and skip window match
//! everywhere. It grows from the control's bottom edge, slides the last few points into place
//! and ignores the pointer.

use crate::core::{c, nr_rect};
use crate::motion::{Move, Pose, now, number};
use crate::text::width;
use crate::theme::{Face, Token, color};
use gpui::{
    App, Bounds, Context, IntoElement, PathBuilder, Pixels, Point, Render, Task, Window, canvas,
    div, point, prelude::*, px,
};
use northroom_core::ffi::NrMotion;
use northroom_core::window::tooltip::{
    nr_tooltip_due, nr_tooltip_leave, nr_tooltip_place, nr_tooltip_rest,
};
use std::time::Duration;

/// Space beside the body that the diamond's point reaches into.
const RISE: f32 = 6.0;
/// How far inside the body's control-facing edge the diamond's centre sits.
const DIAMOND_IN: f32 = 5.0;

fn face() -> Face {
    Face::sans(12.0, 400.0)
}

/// A tooltip being drawn. Its panel covers the body and the diamond's space.
#[derive(Clone, Debug)]
struct Tip {
    text: String,
    /// The panel's resting top-left in the window, and its size.
    at: Point<Pixels>,
    width: f32,
    height: f32,
    /// The diamond's point, from the panel's left.
    pointer: f32,
    /// The text's line box, from the body's top-left.
    words: Bounds<Pixels>,
    origin: Point<Pixels>,
    motion: Move,
    /// Leaving in place: it shrinks and fades without sliding.
    leaving: bool,
}

impl Tip {
    fn pose(&self) -> Pose {
        let slide = number(NrMotion::Slide) as f32;
        // Slides the last few points down from the control.
        let slide = if self.leaving { 0.0 } else { -slide };
        Pose::of(
            &self.motion.frame(),
            self.at,
            point(px(0.0), px(slide)),
            self.origin,
        )
    }
}

/// A window's tooltip, plus any still leaving.
pub struct Tooltips {
    showing: Option<Tip>,
    leaving: Vec<Tip>,
    waiting: Option<Task<()>>,
}

impl Tooltips {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            showing: None,
            leaving: Vec::new(),
            waiting: None,
        }
    }

    /// The pointer rests on a control at `anchor` (window points) labelled `text`; its tooltip
    /// shows under it when due. Repeating for the same control does nothing. Resting on another
    /// control hides the current tooltip.
    pub fn rest_on(
        &mut self,
        anchor: Bounds<Pixels>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("{text}\t{anchor:?}");
        let named = c(&key);
        // SAFETY: `named` is NUL-terminated.
        let wait = unsafe { nr_tooltip_rest(named.as_ptr(), now()) };
        if wait < 0.0 {
            return;
        }
        self.take_away(cx);
        let text = text.to_owned();
        self.waiting = Some(cx.spawn_in(window, async move |tooltips, cx| {
            cx.background_executor()
                .timer(Duration::from_secs_f64(wait))
                .await;
            let _ = tooltips.update_in(cx, |tooltips, window, cx| {
                let named = c(&key);
                // SAFETY: `named` is NUL-terminated.
                if unsafe { nr_tooltip_due(named.as_ptr(), now()) } {
                    tooltips.show(&text, anchor, window, cx);
                }
            });
        }));
    }

    /// The pointer left the control, or pressed it.
    pub fn hide(&mut self, cx: &mut Context<Self>) {
        nr_tooltip_leave(now());
        self.waiting = None;
        self.take_away(cx);
    }

    fn take_away(&mut self, cx: &mut Context<Self>) {
        if let Some(mut leaving) = self.showing.take() {
            let pose = leaving.pose();
            leaving.at = pose.at;
            leaving.motion = Move::leaving(NrMotion::Popup);
            leaving.leaving = true;
            self.leaving.push(leaving);
            cx.notify();
        }
    }

    /// Shows the tooltip under the control at `anchor` immediately.
    fn show(
        &mut self,
        text: &str,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.take_away(cx);
        let view_width = f64::from(window.viewport_size().width);
        let placed = nr_tooltip_place(nr_rect(anchor), f64::from(width(text, face())), view_width);
        let body = crate::core::rect(&placed.body);
        let (body_x, body_y) = (f32::from(body.origin.x), f32::from(body.origin.y));
        // The diamond's space sits above the body.
        let top = body_y - RISE;
        // It grows about the control's bottom edge.
        let edge = f32::from(anchor.origin.y + anchor.size.height);
        let pointer = placed.pointer as f32;
        self.showing = Some(Tip {
            text: text.to_owned(),
            at: point(px(body_x), px(top)),
            width: f32::from(body.size.width),
            height: f32::from(body.size.height) + RISE,
            pointer,
            words: crate::core::frame(
                placed.text.x as f32 - body_x,
                placed.text.y as f32 - body_y,
                placed.text.width as f32,
                placed.text.height as f32,
            ),
            origin: point(px(pointer), px(edge - top)),
            motion: Move::entering(NrMotion::Popup),
            leaving: false,
        });
        cx.notify();
    }

    /// The text of the tooltip showing, for tests.
    pub fn shown(&self) -> Option<&str> {
        self.showing.as_ref().map(|tip| tip.text.as_str())
    }

    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        self.leaving.retain(|tip| !tip.motion.gone());
        for tip in self.leaving.iter().chain(&self.showing) {
            draw(tip, window, cx);
        }
        if self
            .leaving
            .iter()
            .chain(&self.showing)
            .any(|tip| tip.motion.runs())
        {
            window.request_animation_frame();
        }
    }
}

/// Draws a tooltip: a 10 pt diamond turned 45° with its centre 5 pt inside the body's
/// control-facing edge (2 pt of its tip shows), the body with 6 px corners, and the text. It
/// fades as one group.
fn draw(tip: &Tip, window: &mut Window, cx: &mut App) {
    let pose = tip.pose();
    crate::layer::group(window, cx, pose.opacity, |window, cx| {
        draw_at(
            tip,
            Pose {
                opacity: 1.0,
                ..pose
            },
            window,
            cx,
        )
    });
}

fn draw_at(tip: &Tip, pose: Pose, window: &mut Window, cx: &mut App) {
    let ground = pose.color(color(Token::Popover));
    let body_top = RISE;
    let body = crate::core::frame(0.0, body_top, tip.width, tip.height - RISE);
    let half = DIAMOND_IN * 2.0_f32.sqrt();
    let middle = body_top + DIAMOND_IN;
    let mut diamond = PathBuilder::fill();
    diamond.move_to(pose.place(tip.pointer, middle - half));
    diamond.line_to(pose.place(tip.pointer + half, middle));
    diamond.line_to(pose.place(tip.pointer, middle + half));
    diamond.line_to(pose.place(tip.pointer - half, middle));
    diamond.close();
    if let Ok(path) = diamond.build() {
        window.paint_path(path, ground);
    }
    window.paint_quad(gpui::fill(pose.rect(body), ground).corner_radii(px(pose.length(6.0))));
    let words = crate::core::offset(tip.words, 0.0, body_top);
    pose.line(
        window,
        cx,
        &tip.text,
        face(),
        color(Token::PopoverForeground),
        words,
        f32::from(words.size.width),
    );
}

impl Render for Tooltips {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.showing.is_none() && self.leaving.is_empty() {
            return div().into_any_element();
        }
        let view = window.viewport_size();
        let this = cx.entity();
        crate::popover::layer(
            canvas(
                |_, _, _| (),
                move |_, (), window, cx| this.update(cx, |tips, cx| tips.paint(window, cx)),
            )
            .w(view.width)
            .h(view.height),
            crate::popover::Layer::Tooltip,
        )
    }
}
