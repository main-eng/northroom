//! Timing and placement for the single tooltip every view shares (`motion::Tooltip`,
//! `motion::place_tooltip`).

use crate::ffi::{text_of, NrRect};
use crate::motion::Tooltip;
use crate::Relock;
use std::ffi::c_char;
use std::sync::Mutex;

static TOOLTIP: Mutex<Option<Tooltip>> = Mutex::new(None);

fn with_tooltip<T>(action: impl FnOnce(&mut Tooltip) -> T) -> T {
    let mut tooltip = TOOLTIP.relock();
    action(tooltip.get_or_insert_with(Tooltip::default))
}

/// The pointer rested on control `key` at `now` (seconds on a monotonic clock). Returns the
/// seconds until its tooltip is due (call `nr_tooltip_due` then), or -1 when it was already
/// resting there. Any showing tooltip is dismissed (`motion::Tooltip::rest`).
///
/// # Safety
/// `key` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_tooltip_rest(key: *const c_char, now: f64) -> f64 {
    // SAFETY: the caller passes a NUL-terminated string.
    let key = unsafe { text_of(key) };
    with_tooltip(|tooltip| tooltip.rest(&key, now * 1000.0)).map_or(-1.0, |wait| wait / 1000.0)
}

/// Called when the wait for `key` ends at `now`. Returns whether its tooltip shows.
///
/// # Safety
/// `key` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_tooltip_due(key: *const c_char, now: f64) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let key = unsafe { text_of(key) };
    with_tooltip(|tooltip| tooltip.due(&key, now * 1000.0))
}

/// The pointer left or pressed the control at `now`. Returns whether a tooltip was showing, for
/// the shell to remove.
#[no_mangle]
pub extern "C" fn nr_tooltip_leave(now: f64) -> bool {
    with_tooltip(|tooltip| tooltip.leave(now * 1000.0))
}

/// Where a tooltip is drawn (`motion::place_tooltip`), in points from the top-left of its
/// control's space: the `body`, its text starting at `text` (one 16 pt line from the top, no
/// wider than the body), and the arrow tip `pointer` from the body's left.
#[repr(C)]
pub struct NrTooltipPlace {
    pub body: NrRect,
    pub text: NrRect,
    pub pointer: f64,
}

/// Places the tooltip for the control at `anchor`, with text `words` wide, below it in a space
/// `bounds_width` wide (y down).
#[no_mangle]
pub extern "C" fn nr_tooltip_place(
    anchor: NrRect,
    words: f64,
    bounds_width: f64,
) -> NrTooltipPlace {
    let placed = crate::motion::place_tooltip(
        anchor.x,
        anchor.y,
        anchor.width,
        anchor.height,
        words,
        bounds_width,
    );
    NrTooltipPlace {
        body: NrRect {
            x: placed.x,
            y: placed.y,
            width: placed.width,
            height: placed.height,
        },
        text: NrRect {
            x: placed.text_x,
            y: placed.text_y,
            width: placed.text_width,
            height: crate::motion::TOOLTIP_LINE,
        },
        pointer: placed.pointer,
    }
}
