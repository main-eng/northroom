//! The shape a session's dot takes (`terminal::activity::Sign`) wherever the dot shows. The
//! window animates it (`Window::paint_timed_svg`): the sprite is turned or slid on the GPU before
//! each present, so the view is not redrawn. With reduced motion each sign is drawn still.

use crate::core::inset;
use crate::draw::oval;
use gpui::{
    App, Bounds, Easing, Hsla, Pixels, SharedString, TimedTransformation, Transformation, Window,
    point, px,
};
use northroom_core::terminal::activity::Sign;
use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Duration;

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
    let across = f32::from(dot.size.width);
    let (key, svg, timed) = match sign {
        // One turn per second. 30 fps is smooth at this size and halves the work.
        "comet" => (
            "sign/comet.svg",
            comet(),
            TimedTransformation::rotation(Duration::from_millis(1000), Transformation::default())
                .with_max_fps(30.0),
        ),
        // Hops up and back. At rest it sits where the dot would.
        "nudge" => (
            "sign/dot.svg",
            DOT,
            TimedTransformation::new(Duration::from_millis(700))
                .from(Transformation::default())
                .to(Transformation::translate(point(
                    px(0.0),
                    px(-across * 0.45),
                )))
                .with_easing(Easing::EaseInOut)
                .alternate(),
        ),
        "check" => ("sign/check.svg", CHECK, still()),
        "ring" => ("sign/ring.svg", RING, still()),
        _ => {
            oval(window, dot, color);
            return;
        }
    };
    let _ = window.paint_timed_svg(
        bounds,
        SharedString::from(key),
        Some(svg.as_bytes()),
        timed,
        color,
        cx,
    );
}

fn still() -> TimedTransformation {
    TimedTransformation::new(Duration::ZERO)
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
