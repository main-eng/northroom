//! Scroll routing and scroll distances, decided here instead of in each shell's views.
//!
//! * [`route`]: which of two nested scrollers with different axes takes a wheel or trackpad
//!   event (a column's list inside the row of columns, or a page inside that row). As in
//!   Chromium, a whole gesture, momentum included, goes to one scroller chosen at its start.
//! * [`reveal`]: scroll just far enough to show something.
//! * [`wheel_scroll`], [`wheel_scrolled`]: how far a clicky mouse wheel scrolls whatever is
//!   under it, 40 pt per click.

/// A trackpad gesture's phases, as AppKit numbers them (`NSEvent.Phase`).
pub mod phase {
    pub const BEGAN: u32 = 1;
    pub const STATIONARY: u32 = 2;
    pub const CHANGED: u32 = 4;
    pub const ENDED: u32 = 8;
    pub const CANCELLED: u32 = 16;
    pub const MAY_BEGIN: u32 = 32;
}

/// What a scroller does with one event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    /// Scroll itself, as the platform scroller would.
    Own,
    /// Pass the event unchanged to the enclosing scroller.
    HandOn,
    /// Hold the event until the gesture's direction is known.
    Hold,
}

/// What a scroller remembers of the gesture in progress. The shell carries it between events
/// across the FFI (`ffi::NrLatch`), so keep the variant order.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Latch {
    /// No gesture.
    #[default]
    Free,
    /// This scroller owns the gesture, through its last momentum event.
    Own,
    /// The outer scroller owns the gesture.
    HandingOn,
    /// The fingers are down but have not moved, so the first event is held.
    Undecided,
}

/// One wheel or trackpad event: its travel in points and its phases.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Scroll {
    pub dx: f64,
    pub dy: f64,
    /// The gesture's phase; 0 for a mouse wheel and for momentum.
    pub phase: u32,
    /// The momentum's phase after the fingers lift; 0 otherwise.
    pub momentum: u32,
}

impl Scroll {
    fn moves(&self) -> bool {
        self.dx != 0.0 || self.dy != 0.0
    }

    /// Whether the event moves more horizontally than vertically. A tie counts as vertical, the
    /// common scrolling axis.
    fn across(&self) -> bool {
        self.dx.abs() > self.dy.abs()
    }
}

/// Which scroller takes `scroll`, for a horizontal (`sideways`) or vertical scroller, and the
/// latch it keeps afterwards.
///
/// * A mouse wheel click has no phase, so each click is decided alone by its larger axis.
/// * Resting fingers (`MAY_BEGIN`) scroll nothing. The scroller under them takes the event, as
///   the platform's does to show its scroll bars, and no gesture is latched.
/// * A gesture that begins without travel is held until the first event that moves. That
///   event's larger axis decides, and the decision holds through `ENDED` and all momentum.
pub fn route(scroll: Scroll, sideways: bool, latch: Latch) -> (Route, Latch) {
    // The owner by the event's larger axis, in case its travel decides.
    let travel = if scroll.across() == sideways {
        Latch::Own
    } else {
        Latch::HandingOn
    };
    if scroll.phase == 0 && scroll.momentum == 0 {
        // A wheel click, or a device that reports no gesture phases.
        let chosen = if scroll.moves() { travel } else { Latch::Own };
        return (follow(chosen), Latch::Free);
    }
    if scroll.momentum != 0 {
        return route_momentum(scroll, latch);
    }
    route_gesture(scroll, travel, latch)
}

/// Momentum after the fingers lift belongs to whoever owned the gesture, to its last event.
fn route_momentum(scroll: Scroll, latch: Latch) -> (Route, Latch) {
    let ended = scroll.momentum & (phase::ENDED | phase::CANCELLED) != 0;
    (follow(latch), if ended { Latch::Free } else { latch })
}

/// Where an event goes for the gesture `latch` records.
fn follow(latch: Latch) -> Route {
    match latch {
        Latch::HandingOn => Route::HandOn,
        Latch::Free | Latch::Own | Latch::Undecided => Route::Own,
    }
}

/// An event with fingers on the pad. If it is the first to move, its `travel` decides.
fn route_gesture(scroll: Scroll, travel: Latch, latch: Latch) -> (Route, Latch) {
    if scroll.phase & phase::MAY_BEGIN != 0 {
        return (Route::Own, Latch::Free);
    }
    if scroll.phase & phase::BEGAN != 0 {
        if !scroll.moves() {
            return (Route::Hold, Latch::Undecided);
        }
        return (follow(travel), travel);
    }
    if scroll.phase & (phase::ENDED | phase::CANCELLED) != 0 {
        // Fingers lifted. A gesture that never moved is this scroller's: it has seen both ends
        // and scrolls nothing. The latch stays for the momentum.
        let latch = match latch {
            Latch::Undecided | Latch::Free => Latch::Own,
            Latch::Own | Latch::HandingOn => latch,
        };
        return (follow(latch), latch);
    }
    // CHANGED or STATIONARY.
    match latch {
        Latch::Undecided | Latch::Free if scroll.moves() => (follow(travel), travel),
        Latch::Undecided => (Route::Hold, Latch::Undecided),
        other => (follow(other), other),
    }
}

/// The offset that shows `start..end` in a view of `view_start..view_start + view_length` over
/// `content_length`. It aligns the far edge, then the near one, so anything longer than the
/// view shows its start. Never past either end.
pub fn reveal(view_start: f64, view_length: f64, content_length: f64, start: f64, end: f64) -> f64 {
    let past = (end - (view_start + view_length)).max(0.0);
    let offset = view_start + past - (view_start - (start - past)).max(0.0);
    clamp(offset, view_length, content_length)
}

/// `offset` clamped between 0 and the furthest a `view_length` view can scroll over
/// `content_length`.
pub fn clamp(offset: f64, view_length: f64, content_length: f64) -> f64 {
    offset.min(content_length - view_length).max(0.0)
}

/// The FFI units for wheel travel, in points or clicks.
pub use crate::ffi::{NR_WHEEL_CLICKS as WHEEL_CLICKS, NR_WHEEL_POINTS as WHEEL_POINTS};
/// How far one wheel click scrolls, matching what Chromium gives a page for a Mac wheel click
/// (`kScrollbarPixelsPerCocoaTick`). AppKit scrollers move about a quarter as far.
pub const WHEEL_CLICK_POINTS: f64 = 40.0;

/// Wheel travel `delta` in `unit` (`WHEEL_POINTS` or `WHEEL_CLICKS`), converted to points. Any
/// other unit is treated as points.
pub fn wheel_points(delta: f64, unit: u32) -> f64 {
    match unit {
        WHEEL_CLICKS => delta * WHEEL_CLICK_POINTS,
        _ => delta,
    }
}

/// The new offset of an app-drawn scroller at `offset` after the wheel moved `delta` in `unit`
/// (positive is down or right), clamped to the scrollable range of a `view_length` view over
/// `content_length`.
pub fn wheel_scrolled(
    offset: f64,
    delta: f64,
    unit: u32,
    view_length: f64,
    content_length: f64,
) -> f64 {
    clamp(
        offset + wheel_points(delta, unit),
        view_length,
        content_length,
    )
}

/// The same for a platform-drawn scroller. A clicky wheel scrolls as `wheel_scrolled` says;
/// anything else returns `None` and is left to the platform scroller (trackpad points, with
/// their momentum and elastic ends).
pub fn wheel_scroll(
    offset: f64,
    delta: f64,
    unit: u32,
    view_length: f64,
    content_length: f64,
) -> Option<f64> {
    (unit == WHEEL_CLICKS).then(|| wheel_scrolled(offset, delta, unit, view_length, content_length))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(dx: f64, dy: f64) -> Scroll {
        Scroll {
            dx,
            dy,
            phase: 0,
            momentum: 0,
        }
    }

    fn touch(dx: f64, dy: f64, phase: u32) -> Scroll {
        Scroll {
            dx,
            dy,
            phase,
            momentum: 0,
        }
    }

    fn glide(dx: f64, dy: f64, momentum: u32) -> Scroll {
        Scroll {
            dx,
            dy,
            phase: 0,
            momentum,
        }
    }

    /// Runs a whole gesture through one scroller and returns its route for each event.
    fn play(events: &[Scroll], sideways: bool) -> Vec<Route> {
        let mut latch = Latch::Free;
        events
            .iter()
            .map(|&event| {
                let (route, next) = route(event, sideways, latch);
                latch = next;
                route
            })
            .collect()
    }

    #[test]
    fn a_wheel_click_goes_by_its_larger_part() {
        // A column's list (up and down) inside the row of columns (sideways).
        assert_eq!(
            route(wheel(0.0, -40.0), false, Latch::Free),
            (Route::Own, Latch::Free)
        );
        assert_eq!(
            route(wheel(-40.0, 0.0), false, Latch::Free),
            (Route::HandOn, Latch::Free)
        );
        assert_eq!(
            route(wheel(-40.0, 0.0), true, Latch::Free),
            (Route::Own, Latch::Free)
        );
        assert_eq!(
            route(wheel(0.0, -40.0), true, Latch::Free),
            (Route::HandOn, Latch::Free)
        );
        // A tie counts as vertical.
        assert_eq!(route(wheel(10.0, 10.0), false, Latch::Free).0, Route::Own);
        assert_eq!(route(wheel(10.0, 10.0), true, Latch::Free).0, Route::HandOn);
        // A click with no travel is the scroller's own and latches nothing.
        assert_eq!(
            route(wheel(0.0, 0.0), true, Latch::Free),
            (Route::Own, Latch::Free)
        );
    }

    #[test]
    fn a_wheel_click_never_latches() {
        assert_eq!(
            play(
                &[wheel(-40.0, 0.0), wheel(0.0, -40.0), wheel(-40.0, 0.0)],
                false
            ),
            [Route::HandOn, Route::Own, Route::HandOn]
        );
    }

    #[test]
    fn a_sideways_gesture_over_a_list_is_handed_on_whole() {
        let gesture = [
            touch(0.0, 0.0, phase::BEGAN),
            touch(-6.0, 1.0, phase::CHANGED),
            // Later it wanders more down than across: it stays with the row of columns.
            touch(-2.0, -9.0, phase::CHANGED),
            touch(0.0, 0.0, phase::ENDED),
            glide(-3.0, -8.0, phase::BEGAN),
            glide(-1.0, -4.0, phase::CHANGED),
            glide(0.0, 0.0, phase::ENDED),
        ];
        assert_eq!(
            play(&gesture, false),
            [
                Route::Hold,
                Route::HandOn,
                Route::HandOn,
                Route::HandOn,
                Route::HandOn,
                Route::HandOn,
                Route::HandOn
            ]
        );
        // The row of columns takes the same gesture as its own, the first event included.
        assert_eq!(
            play(&gesture, true),
            [
                Route::Hold,
                Route::Own,
                Route::Own,
                Route::Own,
                Route::Own,
                Route::Own,
                Route::Own
            ]
        );
    }

    #[test]
    fn a_gesture_up_and_down_stays_with_the_list() {
        let gesture = [
            touch(0.0, -3.0, phase::BEGAN),
            touch(4.0, -2.0, phase::CHANGED),
            touch(0.0, 0.0, phase::ENDED),
            glide(5.0, -1.0, phase::BEGAN),
            glide(0.0, 0.0, phase::ENDED),
        ];
        assert_eq!(play(&gesture, false), [Route::Own; 5]);
        assert_eq!(play(&gesture, true), [Route::HandOn; 5]);
    }

    #[test]
    fn the_latch_is_let_go_when_the_momentum_ends() {
        let mut latch = Latch::Free;
        for event in [
            touch(-5.0, 0.0, phase::BEGAN),
            touch(0.0, 0.0, phase::ENDED),
            glide(-2.0, 0.0, phase::BEGAN),
        ] {
            latch = route(event, false, latch).1;
            assert_eq!(latch, Latch::HandingOn);
        }
        assert_eq!(
            route(glide(0.0, 0.0, phase::ENDED), false, latch),
            (Route::HandOn, Latch::Free)
        );
        // The next gesture is decided afresh.
        assert_eq!(
            route(touch(0.0, -5.0, phase::BEGAN), false, Latch::Free),
            (Route::Own, Latch::Own)
        );
    }

    #[test]
    fn a_new_gesture_overrides_a_latch_left_by_one_without_momentum() {
        // Fingers lifted with no momentum: the latch is still set when the next gesture begins.
        let (_, latch) = route(touch(0.0, 0.0, phase::ENDED), false, Latch::HandingOn);
        assert_eq!(latch, Latch::HandingOn);
        assert_eq!(
            route(touch(0.0, -4.0, phase::BEGAN), false, latch),
            (Route::Own, Latch::Own)
        );
        assert_eq!(
            route(touch(0.0, 0.0, phase::BEGAN), false, latch),
            (Route::Hold, Latch::Undecided)
        );
    }

    #[test]
    fn resting_fingers_scroll_nothing_and_latch_nothing() {
        assert_eq!(
            route(touch(0.0, 0.0, phase::MAY_BEGIN), false, Latch::HandingOn),
            (Route::Own, Latch::Free)
        );
        assert_eq!(
            route(touch(0.0, 0.0, phase::CANCELLED), false, Latch::Free),
            (Route::Own, Latch::Own)
        );
    }

    #[test]
    fn a_gesture_that_never_travels_is_the_scrollers_own() {
        assert_eq!(
            play(
                &[
                    touch(0.0, 0.0, phase::BEGAN),
                    touch(0.0, 0.0, phase::CHANGED),
                    touch(0.0, 0.0, phase::ENDED)
                ],
                false
            ),
            [Route::Hold, Route::Hold, Route::Own]
        );
    }

    #[test]
    fn reveal_scrolls_only_as_far_as_shows_the_span() {
        // Five 240 pt columns and a 640 pt pane in a 1280 pt window, scrolled to 320.
        // The first column is off to the left and comes back to the edge.
        assert_eq!(reveal(320.0, 1280.0, 1840.0, 0.0, 240.0), 0.0);
        // The pane at the far end is past the right edge.
        assert_eq!(reveal(0.0, 1280.0, 1840.0, 1200.0, 1840.0), 560.0);
        // Something already in view does not scroll.
        assert_eq!(reveal(100.0, 1280.0, 1840.0, 240.0, 480.0), 100.0);
        // Something longer than the view shows its start.
        assert_eq!(reveal(0.0, 300.0, 2000.0, 500.0, 1000.0), 500.0);
        // Never past the end, nor before the start.
        assert_eq!(reveal(0.0, 1280.0, 1300.0, 1290.0, 1400.0), 20.0);
        assert_eq!(reveal(50.0, 1280.0, 1000.0, 0.0, 10.0), 0.0);
    }

    #[test]
    fn clamp_holds_an_offset_inside_what_can_be_scrolled() {
        assert_eq!(clamp(325.0, 1280.0, 1600.0), 320.0);
        assert_eq!(clamp(-5.0, 1280.0, 1600.0), 0.0);
        assert_eq!(clamp(40.0, 1280.0, 900.0), 0.0);
    }

    #[test]
    fn a_click_of_the_wheel_travels_forty_points_and_a_trackpad_its_own() {
        assert_eq!(wheel_points(1.0, WHEEL_CLICKS), 40.0);
        assert_eq!(wheel_points(-2.0, WHEEL_CLICKS), -80.0);
        // A faster wheel sends fractional and multiple clicks.
        assert_eq!(wheel_points(0.5, WHEEL_CLICKS), 20.0);
        assert_eq!(wheel_points(12.5, WHEEL_POINTS), 12.5);
    }

    #[test]
    fn a_scroller_the_app_draws_moves_by_the_wheel_held_to_its_ends() {
        assert_eq!(wheel_scrolled(0.0, 1.0, WHEEL_CLICKS, 300.0, 1200.0), 40.0);
        assert_eq!(
            wheel_scrolled(40.0, 3.0, WHEEL_CLICKS, 300.0, 1200.0),
            160.0
        );
        assert_eq!(
            wheel_scrolled(80.0, -1.0, WHEEL_CLICKS, 300.0, 1200.0),
            40.0
        );
        assert_eq!(wheel_scrolled(40.0, -2.0, WHEEL_CLICKS, 300.0, 1200.0), 0.0);
        assert_eq!(
            wheel_scrolled(880.0, 2.0, WHEEL_CLICKS, 300.0, 1200.0),
            900.0
        );
        assert_eq!(
            wheel_scrolled(100.0, -7.5, WHEEL_POINTS, 300.0, 1200.0),
            92.5
        );
        assert_eq!(
            wheel_scrolled(0.0, 1.0, WHEEL_CLICKS, 300.0, 250.0),
            0.0,
            "what fits does not move"
        );
    }

    #[test]
    fn a_platform_scroller_is_moved_only_for_a_wheel_that_turns_in_clicks() {
        assert_eq!(
            wheel_scroll(0.0, 1.0, WHEEL_CLICKS, 736.0, 1636.0),
            Some(40.0)
        );
        assert_eq!(
            wheel_scroll(880.0, 2.0, WHEEL_CLICKS, 736.0, 1636.0),
            Some(900.0)
        );
        assert_eq!(
            wheel_scroll(0.0, 1.0, WHEEL_CLICKS, 736.0, 700.0),
            Some(0.0)
        );
        // Trackpad points, momentum included, are left to the platform scroller.
        assert_eq!(wheel_scroll(0.0, 12.0, WHEEL_POINTS, 736.0, 1636.0), None);
    }
}
