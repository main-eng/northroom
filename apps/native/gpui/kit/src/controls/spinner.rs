//! The spinner: the circle-notch glyph rotated to the core's angle for the current time
//! (`nr_motion_spin`), redrawn every frame while the core says it spins (`nr_motion_spins`).
//! `reduced` is the system's reduce-motion setting.

use crate::icons::{Glyph, drawing};
use crate::nr::{nr_motion_spin, nr_motion_spins};
use crate::theme::{Token, color};
use gpui::{App, Bounds, Pixels, Radians, ScaledPixels, TransformationMatrix, Window, point};
use std::sync::OnceLock;
use std::time::Instant;

const GLYPH: Glyph = Glyph::CircleNotch;

/// Seconds on a monotonic clock; the spinner's angle is a function of it.
pub fn clock() -> f64 {
    static STARTED: OnceLock<Instant> = OnceLock::new();
    STARTED.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// Draws the spinner filling `bounds` in `ink` at `seconds` on `clock`, and requests the next
/// frame while it spins.
pub fn paint(
    window: &mut Window,
    cx: &App,
    bounds: Bounds<Pixels>,
    ink: Token,
    seconds: f64,
    reduced: bool,
) {
    let turned = nr_motion_spin(seconds, reduced);
    let (key, svg) = drawing(GLYPH);
    let scale = window.scale_factor();
    let centre = bounds.center();
    let middle = point(
        ScaledPixels(f32::from(centre.x) * scale),
        ScaledPixels(f32::from(centre.y) * scale),
    );
    let back = point(ScaledPixels(-middle.x.0), ScaledPixels(-middle.y.0));
    let turn = TransformationMatrix::unit()
        .translate(middle)
        .rotate(Radians((turned * std::f64::consts::TAU) as f32))
        .translate(back);
    let _ = window.paint_svg(bounds, key, Some(&svg), turn, color(ink), cx);
    if nr_motion_spins(reduced) {
        window.request_animation_frame();
    }
}
