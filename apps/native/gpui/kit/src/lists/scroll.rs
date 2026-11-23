//! An app-drawn vertical scroller. The core owns the offset, clamped at the ends with no
//! bounce, and the overlay scrollbar (`scroller.rs`, `nr_scroller_*`). This module feeds GPUI
//! scroll events to the core and draws the scrollbar where the core places it.
//!
//! When scrollers nest across directions (a column inside the row of columns), each takes only
//! its own gestures. The core picks the owner when a gesture first moves (`nr_scroll_route`)
//! and keeps it through momentum (`Route`).

use crate::core::{nr_rect, rect};
use crate::draw::rounded;
use crate::theme::{Token, color};
use gpui::{Bounds, Pixels, Point, ScrollDelta, ScrollEvent, TouchPhase, Window};
// AppKit's gesture phase numbers, as the core's routing reads them.
use northroom_core::ffi::{NrLatch, NrRect, NrRoute, nr_appearance_is_dark, nr_scroll_route};
use northroom_core::scroll::route::phase::{BEGAN, CHANGED, ENDED};
use northroom_core::window::ffi::{
    NrScroller, NrScrollerBar, NrScrollerTravel, nr_scroller_bar, nr_scroller_free,
    nr_scroller_held, nr_scroller_new, nr_scroller_offset, nr_scroller_pointer, nr_scroller_press,
    nr_scroller_release, nr_scroller_reveal, nr_scroller_scroll_to, nr_scroller_set_extent,
    nr_scroller_tick, nr_scroller_wheel,
};

/// One of the core's scrollers. Times are `motion::now()`.
#[derive(Debug)]
pub struct Scroller(*mut NrScroller);

impl Default for Scroller {
    fn default() -> Self {
        Self(nr_scroller_new())
    }
}

impl Drop for Scroller {
    fn drop(&mut self) {
        // SAFETY: the scroller is this one's and is not used again.
        unsafe { nr_scroller_free(self.0) };
    }
}

impl Scroller {
    /// Sets the visible `shown` out of `length` and clamps the offset to the new range.
    pub fn set_extent(&mut self, shown: f32, length: f32) {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_set_extent(self.0, f64::from(shown), f64::from(length)) };
    }

    /// `set_extent` for lengths beyond `f32` point precision, such as tables of millions of rows.
    pub fn set_long_extent(&mut self, shown: f64, length: f64) {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_set_extent(self.0, shown, length) };
    }

    pub fn offset(&self) -> f32 {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_offset(self.0) as f32 }
    }

    /// The offset at full precision (see `set_long_extent`).
    pub fn long_offset(&self) -> f64 {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_offset(self.0) }
    }

    /// Jumps to `offset`, clamped to the ends.
    pub fn scroll_to(&mut self, offset: f32) {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_scroll_to(self.0, f64::from(offset)) };
    }

    /// Scrolls just far enough to show `start..end`.
    pub fn reveal(&mut self, start: f32, end: f32) {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_reveal(self.0, f64::from(start), f64::from(end)) };
    }

    /// Applies a wheel or trackpad event at `now`. True when it moved.
    pub fn wheel(&mut self, event: &ScrollEvent, now: f64) -> bool {
        let (travel, up) = match event.delta {
            ScrollDelta::Lines(delta) => (NrScrollerTravel::Clicks, delta.y),
            ScrollDelta::Pixels(delta) => (NrScrollerTravel::Points, f32::from(delta.y)),
        };
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_wheel(self.0, travel, -f64::from(up), now) }
    }

    /// True while the scrollbar is visible at `now`, so the view requests another frame.
    pub fn tick(&self, now: f64) -> bool {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_tick(self.0, now) }
    }

    /// Draws the overlay scrollbar over `view` at `now`: dark on light windows and light on
    /// dark, like the system's.
    pub fn paint_bar(&self, window: &mut Window, view: Bounds<Pixels>, now: f64) {
        // SAFETY: the scroller lives as long as this.
        let bar = unsafe { nr_scroller_bar(self.0, nr_rect(view), now) };
        Self::paint_placed(window, bar);
    }

    /// Draws a horizontal scroller's bar along the bottom of `view`. The core places it as a
    /// vertical bar on `view` transposed across its diagonal.
    pub fn paint_bar_across(&self, window: &mut Window, view: Bounds<Pixels>, now: f64) {
        fn turned(rect: &NrRect) -> NrRect {
            NrRect {
                x: rect.y,
                y: rect.x,
                width: rect.height,
                height: rect.width,
            }
        }
        // SAFETY: the scroller lives as long as this.
        let mut bar = unsafe { nr_scroller_bar(self.0, turned(&nr_rect(view)), now) };
        bar.track = turned(&bar.track);
        bar.knob = turned(&bar.knob);
        Self::paint_placed(window, bar);
    }

    fn paint_placed(window: &mut Window, bar: NrScrollerBar) {
        if bar.knob_opacity <= 0.0 {
            return;
        }
        let ink = if nr_appearance_is_dark() {
            color(Token::Foreground)
        } else {
            color(Token::Ink)
        };
        if bar.track_opacity > 0.0 {
            crate::draw::fill(
                window,
                rect(&bar.track),
                ink.alpha(bar.track_opacity as f32),
            );
        }
        let knob = rect(&bar.knob);
        rounded(
            window,
            knob,
            ink.alpha(bar.knob_opacity as f32),
            f32::from(knob.size.width.min(knob.size.height)) / 2.0,
        );
    }

    /// Handles a pointer move to `at` over `view`. True when the scrollbar changed.
    pub fn pointer(&mut self, at: Point<Pixels>, view: Bounds<Pixels>, now: f64) -> bool {
        let (x, y) = (f64::from(at.x), f64::from(at.y));
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_pointer(self.0, x, y, nr_rect(view), now) }
    }

    /// Handles a press at `at` over `view`: the knob grabs, the track jumps the knob there.
    /// True when the scrollbar took the press.
    pub fn press(&mut self, at: Point<Pixels>, view: Bounds<Pixels>, now: f64) -> bool {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_press(self.0, f64::from(at.y), nr_rect(view), now) }
    }

    /// Releases the knob.
    pub fn release(&mut self, now: f64) -> bool {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_release(self.0, now) }
    }

    pub fn is_held(&self) -> bool {
        // SAFETY: the scroller lives as long as this.
        unsafe { nr_scroller_held(self.0) }
    }
}

/// Routes wheel and trackpad gestures for a one-direction scroller (`nr_scroll_route`). Holds
/// the core's latch for the gesture and any first event whose direction is not yet known.
#[derive(Debug)]
pub struct Route {
    sideways: bool,
    latch: NrLatch,
    held: Option<ScrollEvent>,
    /// Between a gesture's `Started` and `Ended`. A `Moved` outside this is momentum.
    touching: bool,
}

impl Route {
    pub fn new(sideways: bool) -> Self {
        Self {
            sideways,
            latch: NrLatch::Free,
            held: None,
            touching: false,
        }
    }

    /// Whether this scroller owns an ongoing trackpad gesture, including momentum, wherever the
    /// pointer goes. Wheel clicks are independent and never latch.
    pub fn owns_gesture(&self) -> bool {
        self.latch == NrLatch::Own
    }

    /// The events this scroller should apply: `event` plus any held before it. Empty when the
    /// gesture belongs to the enclosing scroller or is still held.
    pub fn own(&mut self, event: &ScrollEvent) -> Vec<ScrollEvent> {
        let (dx, dy, phase, momentum) = match event.delta {
            ScrollDelta::Lines(delta) => (f64::from(delta.x), f64::from(delta.y), 0, 0),
            ScrollDelta::Pixels(delta) => {
                let (dx, dy) = (f64::from(f32::from(delta.x)), f64::from(f32::from(delta.y)));
                match event.touch_phase {
                    TouchPhase::Started => {
                        self.touching = true;
                        (dx, dy, BEGAN, 0)
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled => {
                        self.touching = false;
                        (dx, dy, ENDED, 0)
                    }
                    TouchPhase::Moved if self.touching => (dx, dy, CHANGED, 0),
                    TouchPhase::Moved => (dx, dy, 0, CHANGED),
                }
            }
        };
        let answer = nr_scroll_route(dx, dy, phase, momentum, self.sideways, self.latch);
        self.latch = answer.latch;
        match answer.route {
            NrRoute::Hold => {
                self.held.get_or_insert_with(|| event.clone());
                Vec::new()
            }
            NrRoute::HandOn => {
                self.held = None;
                Vec::new()
            }
            NrRoute::Own => self
                .held
                .take()
                .into_iter()
                .chain([event.clone()])
                .collect(),
        }
    }
}
