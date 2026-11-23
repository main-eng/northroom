//! A vertical scroller the app draws itself, for the list, the columns and every other
//! scrolling view. A clicky wheel moves 40 pt per click (`scroll::route::wheel_scrolled`);
//! trackpad travel and the system's momentum pass through as they come. Both are clamped to
//! the ends, with no overscroll or bounce. The overlay scrollbar shows while scrolling and
//! fades afterwards, as on macOS, and its knob can be dragged.
//!
//! Positions are points along the scroll axis, frames are in window coordinates, and times are
//! seconds on a monotonic clock.

use crate::board::Frame;
use crate::scroll::route::{reveal, wheel_scrolled, WHEEL_CLICKS};

/// The overlay scrollbar shows this long after the last move, then fades for `FADE_SECONDS`.
pub const SHOWN_SECONDS: f64 = 0.8;
pub const FADE_SECONDS: f64 = 0.25;
/// The overlay scrollbar's knob, as macOS draws it: its thickness (more while the pointer is
/// over the track), its inset from the scroller's edge and ends, and its minimum length
/// `KNOB_LEAST`. It is fully rounded.
pub const KNOB: f64 = 7.0;
pub const KNOB_WIDE: f64 = 11.0;
pub const KNOB_IN: f64 = 2.0;
pub const KNOB_LEAST: f64 = 18.0;
/// Width of the track at the scroller's trailing edge where the pointer can grab the knob.
pub const TRACK: f64 = 15.0;
/// Opacity of the knob and the track background when the scrollbar is fully shown.
pub const KNOB_OPACITY: f64 = 0.5;
pub const TRACK_OPACITY: f64 = 0.05;

/// The knob's start and length on a track `track` long, for a view showing `shown` of
/// `length`, scrolled to `offset`.
pub fn knob(track: f64, shown: f64, length: f64, offset: f64) -> (f64, f64) {
    let most = (length - shown).max(0.0);
    let long = (track * shown / length.max(shown))
        .max(KNOB_LEAST)
        .min(track);
    let start = if most > 0.0 {
        (track - long) * (offset / most).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (start, long)
}

/// The kind of a wheel or trackpad event, as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Travel {
    /// A wheel that scrolls in clicks.
    Clicks,
    /// Trackpad points: finger travel, or the system's momentum after the fingers lift.
    Points,
}

/// How to draw the overlay scrollbar at one moment: the knob, and the track background while
/// the pointer is on the track, each with an opacity (0 means not drawn).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub knob: Frame,
    pub knob_opacity: f64,
    pub track: Frame,
    pub track_opacity: f64,
}

/// A scroller's state.
#[derive(Clone, Debug, Default)]
pub struct Scroller {
    /// The scroll offset, clamped to the ends.
    offset: f64,
    shown: f64,
    length: f64,
    /// When it last moved, which drives the scrollbar's fade.
    moved: Option<f64>,
    /// The pointer is over the scrollbar's track.
    over_track: bool,
    /// While the knob is held: where the pointer grabbed it and the offset at that time.
    held: Option<(f64, f64)>,
}

impl Scroller {
    /// Sets the view to show `shown` of `length`, clamping the offset to the new ends.
    pub fn set_extent(&mut self, shown: f64, length: f64) {
        self.shown = shown;
        self.length = length;
        self.offset = self.offset.clamp(0.0, self.most());
    }

    fn most(&self) -> f64 {
        (self.length - self.shown).max(0.0)
    }

    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// Jumps to `offset`, clamped to the ends, as a programmatic scroll.
    pub fn scroll_to(&mut self, offset: f64) {
        self.offset = offset.clamp(0.0, self.most());
    }

    /// Scrolls just far enough to show `start..end` (`scroll::route::reveal`).
    pub fn reveal(&mut self, start: f64, end: f64) {
        self.scroll_to(reveal(self.offset, self.shown, self.length, start, end));
    }

    /// Handles a wheel or trackpad event at `now` that scrolls `down` clicks or points. Returns
    /// true when it moved.
    pub fn wheel(&mut self, travel: Travel, down: f64, now: f64) -> bool {
        let before = self.offset;
        self.offset = match travel {
            Travel::Clicks => {
                wheel_scrolled(self.offset, down, WHEEL_CLICKS, self.shown, self.length)
            }
            Travel::Points => (self.offset + down).clamp(0.0, self.most()),
        };
        let moved = self.offset != before;
        if moved {
            self.moved = Some(now);
        }
        moved
    }

    /// True while the scrollbar is still visible at `now`, so the view requests another frame.
    pub fn tick(&self, now: f64) -> bool {
        self.bar_strength(now) > 0.0
    }

    /// The scrollbar's opacity at `now`: full while scrolling, while the knob is held or while the
    /// pointer is over the track, then fading.
    fn bar_strength(&self, now: f64) -> f64 {
        if self.most() <= 0.0 {
            return 0.0;
        }
        if self.held.is_some() || self.over_track {
            return 1.0;
        }
        let Some(moved) = self.moved else { return 0.0 };
        let since = (now - moved).max(0.0);
        if since <= SHOWN_SECONDS {
            1.0
        } else {
            1.0 - ((since - SHOWN_SECONDS) / FADE_SECONDS).min(1.0)
        }
    }

    /// The scrollbar track along the trailing edge of `view`.
    fn track(view: Frame) -> Frame {
        Frame {
            x: view.x + view.width - TRACK,
            y: view.y,
            width: TRACK,
            height: view.height,
        }
    }

    /// The knob's frame over `view`, the scroller's frame in the window.
    fn knob_frame(&self, view: Frame) -> Frame {
        let track = view.height - 2.0 * KNOB_IN;
        let (start, long) = knob(track, self.shown, self.length, self.offset);
        let thick = if self.over_track || self.held.is_some() {
            KNOB_WIDE
        } else {
            KNOB
        };
        Frame {
            x: view.x + view.width - KNOB_IN - thick,
            y: view.y + KNOB_IN + start,
            width: thick,
            height: long,
        }
    }

    /// The overlay scrollbar over `view` at `now`.
    pub fn bar(&self, view: Frame, now: f64) -> Bar {
        let strength = self.bar_strength(now);
        // The track shows under the knob while the pointer is on it.
        let on_track = self.over_track || self.held.is_some();
        Bar {
            knob: self.knob_frame(view),
            knob_opacity: KNOB_OPACITY * strength,
            track: Self::track(view),
            track_opacity: if on_track {
                TRACK_OPACITY * strength
            } else {
                0.0
            },
        }
    }

    /// Handles the pointer moving to (`x`, `y`) over `view`. Returns true when the scrollbar
    /// changed.
    pub fn pointer(&mut self, x: f64, y: f64, view: Frame, now: f64) -> bool {
        if let Some((from, offset)) = self.held {
            let track = view.height - 2.0 * KNOB_IN;
            let room = (track - knob_length(track, self.shown, self.length)).max(1.0);
            self.scroll_to(offset + (y - from) * self.most() / room);
            self.moved = Some(now);
            return true;
        }
        // Like an overlay scroller, the track responds only while the scrollbar is visible.
        let track = Self::track(view);
        let inside =
            x >= track.x && x < track.x + track.width && y >= track.y && y < track.y + track.height;
        let over = self.bar_strength(now) > 0.0 && inside;
        let changed = over != self.over_track;
        self.over_track = over;
        if changed {
            self.moved = Some(now);
        }
        changed
    }

    /// Handles a press at `y` over `view`. A press on the knob grabs it; a press on the track jumps
    /// the knob there. Returns true when the scrollbar took the press.
    pub fn press(&mut self, y: f64, view: Frame, now: f64) -> bool {
        if !self.over_track {
            return false;
        }
        let knob = self.knob_frame(view);
        if !(y >= knob.y && y < knob.y + knob.height) {
            // A press on the track jumps the knob to it, as the system scrollbar does.
            let track = view.height - 2.0 * KNOB_IN;
            let long = knob_length(track, self.shown, self.length);
            let wanted = y - view.y - KNOB_IN - long / 2.0;
            self.scroll_to(wanted / (track - long).max(1.0) * self.most());
        }
        self.held = Some((y, self.offset));
        self.moved = Some(now);
        true
    }

    /// Releases the knob at `now`. Returns true when it was held.
    pub fn release(&mut self, now: f64) -> bool {
        let held = self.held.take().is_some();
        if held {
            self.moved = Some(now);
        }
        held
    }

    pub fn is_held(&self) -> bool {
        self.held.is_some()
    }
}

/// The knob's resting length on a track `track` long.
fn knob_length(track: f64, shown: f64, length: f64) -> f64 {
    knob(track, shown, length, 0.0).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_knob_is_as_long_as_the_share_shown() {
        assert_eq!(knob(100.0, 50.0, 200.0, 0.0), (0.0, 25.0));
        assert_eq!(knob(100.0, 50.0, 200.0, 150.0), (75.0, 25.0));
        // Never shorter than the minimum, even for a long list.
        assert_eq!(knob(100.0, 50.0, 100_000.0, 0.0).1, KNOB_LEAST);
    }

    #[test]
    fn a_wheel_goes_forty_points_a_click_held_to_the_ends() {
        let mut scroller = Scroller::default();
        scroller.set_extent(400.0, 1280.0);
        let mut seen = vec![scroller.offset()];
        for clicks in [1.0, 1.0, -1.0, 3.0, -10.0] {
            scroller.wheel(Travel::Clicks, clicks, 0.0);
            seen.push(scroller.offset());
        }
        assert_eq!(seen, [0.0, 40.0, 80.0, 40.0, 160.0, 0.0]);
        scroller.wheel(Travel::Clicks, 100.0, 0.0);
        assert_eq!(scroller.offset(), 880.0);
    }

    #[test]
    fn a_trackpad_and_its_momentum_stop_at_the_ends_with_nothing_to_spring_back() {
        let mut scroller = Scroller::default();
        scroller.set_extent(400.0, 1280.0);
        // Fingers pulling above the start move nothing.
        assert!(!scroller.wheel(Travel::Points, -30.0, 0.0));
        assert_eq!(scroller.offset(), 0.0);
        // In the middle it goes as far as they travel.
        scroller.wheel(Travel::Points, 123.5, 0.0);
        assert_eq!(scroller.offset(), 123.5);
        // Momentum past the end at 880 stops there.
        let mut seen = Vec::new();
        for step in [500.0, 200.0, 100.0] {
            scroller.wheel(Travel::Points, step, 0.0);
            seen.push(scroller.offset());
        }
        assert_eq!(seen, [623.5, 823.5, 880.0]);
        // Once the scrollbar has faded nothing is left to draw.
        assert!(!scroller.tick(SHOWN_SECONDS + FADE_SECONDS + 0.001));
        assert_eq!(scroller.offset(), 880.0);
    }

    #[test]
    fn the_scrollbar_shows_while_it_moves_then_fades() {
        let mut scroller = Scroller::default();
        let view = Frame {
            x: 100.0,
            y: 50.0,
            width: 300.0,
            height: 404.0,
        };
        scroller.set_extent(400.0, 1600.0);
        assert_eq!(
            scroller.bar(view, 0.0).knob_opacity,
            0.0,
            "nothing moved yet"
        );
        scroller.wheel(Travel::Clicks, 1.0, 1.0);
        let bar = scroller.bar(view, 1.0 + SHOWN_SECONDS);
        assert_eq!(bar.knob_opacity, KNOB_OPACITY);
        assert_eq!(
            bar.track_opacity, 0.0,
            "the track shows only under the pointer"
        );
        // A quarter of the way into the knob's travel of 300, along a track of 400 at 52.
        assert_eq!(
            bar.knob,
            Frame {
                x: 391.0,
                y: 52.0 + 300.0 * 40.0 / 1200.0,
                width: KNOB,
                height: 100.0
            }
        );
        let fading = scroller
            .bar(view, 1.0 + SHOWN_SECONDS + FADE_SECONDS / 2.0)
            .knob_opacity;
        assert!((fading - KNOB_OPACITY / 2.0).abs() < 1e-9, "{fading}");
        assert_eq!(
            scroller
                .bar(view, 1.0 + SHOWN_SECONDS + FADE_SECONDS + 0.001)
                .knob_opacity,
            0.0
        );
        // A list that fits shows none.
        scroller.set_extent(400.0, 300.0);
        assert_eq!(scroller.bar(view, 1.0).knob_opacity, 0.0);
    }

    #[test]
    fn the_knob_is_taken_on_the_track_and_dragged_or_paged_to() {
        let mut scroller = Scroller::default();
        let view = Frame {
            x: 100.0,
            y: 50.0,
            width: 300.0,
            height: 404.0,
        };
        scroller.set_extent(400.0, 1600.0);
        // The track responds only while the scrollbar is visible.
        assert!(!scroller.pointer(395.0, 60.0, view, 0.0));
        assert!(!scroller.press(60.0, view, 0.0));
        scroller.wheel(Travel::Clicks, 1.0, 0.0);
        assert!(scroller.pointer(395.0, 100.0, view, 0.1));
        let bar = scroller.bar(view, 0.1);
        assert_eq!(
            (bar.knob.width, bar.track_opacity),
            (KNOB_WIDE, TRACK_OPACITY)
        );
        // A press on the knob grabs it; pointer travel moves it in proportion to the track.
        assert!(scroller.press(100.0, view, 0.1));
        assert!(scroller.is_held());
        assert!(scroller.pointer(395.0, 175.0, view, 0.2));
        assert_eq!(scroller.offset(), 40.0 + 75.0 * 1200.0 / 300.0);
        assert!(scroller.release(0.3));
        assert!(!scroller.release(0.3));
        // On the track below the knob the knob's middle jumps to the press.
        assert!(scroller.press(52.0 + 350.0, view, 0.4));
        assert_eq!(scroller.offset(), (350.0 - 50.0) / 300.0 * 1200.0);
        // A press away from the track is not the scrollbar's.
        scroller.release(0.5);
        assert!(scroller.pointer(200.0, 100.0, view, 0.5));
        assert!(!scroller.press(100.0, view, 0.5));
    }

    #[test]
    fn a_reveal_scrolls_just_far_enough() {
        let mut scroller = Scroller::default();
        scroller.set_extent(400.0, 1280.0);
        scroller.reveal(500.0, 528.0);
        assert_eq!(scroller.offset(), 128.0);
        scroller.reveal(100.0, 128.0);
        assert_eq!(scroller.offset(), 100.0);
    }
}
