//! The shape a session's dot takes (`terminal::activity::Sign`) wherever the dot shows. A moving
//! sign is drawn at its pose for the current time and asks for the next frame, as the spinner
//! is (`controls::spinner`). With reduced motion each sign is drawn still.

use crate::controls::spinner::clock;
use crate::core::inset;
use crate::draw::oval;
use crate::nr::{nr_motion_spin, nr_motion_spins};
use gpui::{
    App, Bounds, Hsla, Pixels, Radians, ScaledPixels, SharedString, TransformationMatrix, Window,
    point,
};
use northroom_core::terminal::activity::Sign;
use std::f32::consts::TAU;
use std::sync::OnceLock;

/// How long the nudge takes to hop up, and again to come back, in seconds.
const NUDGE_SECONDS: f64 = 0.7;
/// How far the nudge hops, as a share of the dot's width.
const NUDGE_REACH: f32 = 0.45;

/// The bounds `sign` covers for a dot in `dot` (`Sign::growth`), which a caller rings in its own
/// background.
pub fn reach(sign: &str, dot: Bounds<Pixels>) -> Bounds<Pixels> {
    let growth = Sign::named(sign).growth() as f32;
    let grown = f32::from(dot.size.width) * (growth - 1.0) / 2.0;
    inset(dot, -grown, -grown)
}

/// Draws `sign` (a `Sign::name`) for a dot in `dot`, in `color`.
pub fn paint(window: &mut Window, cx: &App, sign: &str, dot: Bounds<Pixels>, color: Hsla) {
    let bounds = reach(sign, dot);
    let reduced = crate::platform::reduce_motion();
    let moving = nr_motion_spins(reduced);
    let scale = window.scale_factor();
    let (key, svg, pose) = match sign {
        // One turn per second, as the spinner turns.
        "comet" => (
            "sign/comet.svg",
            comet(),
            Some(turned(
                bounds,
                scale,
                nr_motion_spin(clock(), reduced) as f32,
            )),
        ),
        // Hops up and back. At rest it sits where the dot would.
        "nudge" => (
            "sign/dot.svg",
            DOT,
            moving.then(|| hopped(f32::from(dot.size.width) * scale, clock())),
        ),
        "check" => ("sign/check.svg", CHECK, None),
        "ring" => ("sign/ring.svg", RING, None),
        _ => {
            oval(window, dot, color);
            return;
        }
    };
    let _ = window.paint_svg(
        bounds,
        SharedString::from(key),
        Some(svg.as_bytes()),
        pose.unwrap_or_else(TransformationMatrix::unit),
        color,
        cx,
    );
    if moving && pose.is_some() {
        window.request_animation_frame();
    }
}

/// `turns` of a turn clockwise about the middle of `bounds`, in device pixels.
fn turned(bounds: Bounds<Pixels>, scale: f32, turns: f32) -> TransformationMatrix {
    let centre = bounds.center();
    let middle = point(
        ScaledPixels(f32::from(centre.x) * scale),
        ScaledPixels(f32::from(centre.y) * scale),
    );
    let back = point(ScaledPixels(-middle.x.0), ScaledPixels(-middle.y.0));
    TransformationMatrix::unit()
        .translate(middle)
        .rotate(Radians(turns * TAU))
        .translate(back)
}

/// The nudge's hop at `seconds` for a dot `across` device pixels wide: up and back, easing in
/// and out each way.
fn hopped(across: f32, seconds: f64) -> TransformationMatrix {
    let passes = seconds / NUDGE_SECONDS;
    let along = passes.fract() as f32;
    let along = if (passes as u64).is_multiple_of(2) {
        along
    } else {
        1.0 - along
    };
    let eased = along * along * (3.0 - 2.0 * along);
    TransformationMatrix::unit().translate(point(
        ScaledPixels(0.0),
        ScaledPixels(-across * NUDGE_REACH * eased),
    ))
}

const DOT: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">\
    <circle cx=\"12\" cy=\"12\" r=\"12\" fill=\"#000\"/></svg>";

const RING: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">\
    <circle cx=\"12\" cy=\"12\" r=\"9.6\" fill=\"none\" stroke=\"#000\" stroke-width=\"3.6\"/></svg>";

const CHECK: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\">\
    <path d=\"M4.5 12.5l5 5L19.5 6.5\" fill=\"none\" stroke=\"#000\" stroke-width=\"3.4\" \
    stroke-linecap=\"round\" stroke-linejoin=\"round\"/></svg>";

/// A ring whose tail fades out over 300° behind a round head, drawn as short arcs of rising
/// opacity.
fn comet() -> &'static str {
    static COMET: OnceLock<String> = OnceLock::new();
    COMET.get_or_init(|| {
        const ARCS: usize = 30;
        const SWEEP: f32 = TAU * 300.0 / 360.0;
        const RADIUS: f32 = 9.6;
        let at = |angle: f32| (12.0 + RADIUS * angle.cos(), 12.0 + RADIUS * angle.sin());
        // Counted clockwise from the top, as the turn goes.
        let start = -TAU / 4.0 - SWEEP;
        let step = SWEEP / ARCS as f32;
        let mut shapes = String::new();
        for arc in 0..ARCS {
            let from = start + step * arc as f32;
            // A sliver of overlap so no seam shows between arcs.
            let to = from + step * 1.15;
            let (x0, y0) = at(from);
            let (x1, y1) = at(to);
            let opacity = ((arc + 1) as f32 / ARCS as f32).powf(1.6);
            shapes.push_str(&format!(
                "<path d=\"M{x0:.3} {y0:.3}A{RADIUS} {RADIUS} 0 0 1 {x1:.3} {y1:.3}\" \
                 stroke-opacity=\"{opacity:.3}\"/>"
            ));
        }
        let (hx, hy) = at(-TAU / 4.0);
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" \
             stroke=\"#000\" stroke-width=\"3.6\">{shapes}\
             <circle cx=\"{hx:.3}\" cy=\"{hy:.3}\" r=\"1.8\" fill=\"#000\" stroke=\"none\"/></svg>"
        )
    })
}
