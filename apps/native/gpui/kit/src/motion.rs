//! Overlay motion. Menus, tooltips and dialogs fade in, grow from 95% about the point they open
//! from and slide the last 8 pt; they leave by fading and shrinking. Durations, curves and poses
//! come from the core (`motion.rs`, `nr_motion_frame`). When a test holds time
//! (`NR_MOTION_AT`, `NR_MOTION_EXIT_AT`), each move is drawn frozen at that moment.
//!
//! GPUI has no transform for quads or text, so `Pose` places every part where the scaled panel
//! would put it and applies the panel's opacity to each colour.

use crate::text::draw_line;
use crate::theme::Face;
use gpui::{App, Bounds, Hsla, Pixels, Point, Window, point, px, size};
use northroom_core::app_ffi::{NrMotionFrame, nr_motion_frame, nr_motion_held};
use northroom_core::ffi::{NrMotion, nr_motion_number};
use std::sync::OnceLock;
use std::time::Instant;

static STARTED: OnceLock<Instant> = OnceLock::new();

/// The monotonic clock, in seconds, shared by the kit's motion and the core's tooltip timing.
pub fn now() -> f64 {
    STARTED.get_or_init(Instant::now).elapsed().as_secs_f64()
}

/// A core motion value: a duration or delay in ms, or the slide in pt.
pub fn number(which: NrMotion) -> f64 {
    nr_motion_number(which)
}

/// An entrance, or an exit when `leaving`, that began at a `now()` time.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    began: f64,
    duration_ms: f64,
    leaving: bool,
}

impl Move {
    pub fn entering(duration: NrMotion) -> Self {
        Self {
            began: now(),
            duration_ms: number(duration),
            leaving: false,
        }
    }

    pub fn leaving(duration: NrMotion) -> Self {
        Self {
            began: now(),
            duration_ms: number(duration),
            leaving: true,
        }
    }

    pub fn frame(&self) -> NrMotionFrame {
        nr_motion_frame(self.began, now(), self.duration_ms, self.leaving)
    }

    /// Whether it needs more frames. A held move is drawn once.
    pub fn runs(&self) -> bool {
        !nr_motion_held(!self.leaving) && !self.frame().done
    }

    /// Whether this is a finished exit. A held exit never finishes.
    pub fn gone(&self) -> bool {
        self.leaving && !nr_motion_held(false) && self.frame().done
    }
}

/// Where a panel is drawn: its resting top-left plus its slide, scaled by `scale` about `origin`
/// (panel-local, from its top-left), at `opacity`.
#[derive(Clone, Copy, Debug)]
pub struct Pose {
    pub at: Point<Pixels>,
    pub origin: Point<Pixels>,
    pub scale: f32,
    pub opacity: f32,
}

impl Pose {
    /// A panel at rest with its top-left at `at`.
    pub fn rest(at: Point<Pixels>) -> Self {
        Self {
            at,
            origin: Point::default(),
            scale: 1.0,
            opacity: 1.0,
        }
    }

    /// The pose `frame` gives a panel resting at `at`, sliding in from `slide` (the full slide,
    /// in window points) and growing about `origin`.
    pub fn of(
        frame: &NrMotionFrame,
        at: Point<Pixels>,
        slide: Point<Pixels>,
        origin: Point<Pixels>,
    ) -> Self {
        let travelled = frame.slide as f32;
        Self {
            at: point(at.x + slide.x * travelled, at.y + slide.y * travelled),
            origin,
            scale: frame.scale as f32,
            opacity: frame.opacity as f32,
        }
    }

    /// A panel-local point in window coordinates.
    pub fn place(&self, x: f32, y: f32) -> Point<Pixels> {
        let (ox, oy) = (f32::from(self.origin.x), f32::from(self.origin.y));
        point(
            self.at.x + px(ox + (x - ox) * self.scale),
            self.at.y + px(oy + (y - oy) * self.scale),
        )
    }

    /// A panel-local rect in window coordinates.
    pub fn rect(&self, local: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: self.place(f32::from(local.origin.x), f32::from(local.origin.y)),
            size: size(
                local.size.width * self.scale,
                local.size.height * self.scale,
            ),
        }
    }

    /// A panel-local length, scaled.
    pub fn length(&self, length: f32) -> f32 {
        length * self.scale
    }

    pub fn color(&self, color: Hsla) -> Hsla {
        color.alpha(color.a * self.opacity)
    }

    /// Draws a line of text in the panel-local line box `frame` (`text::draw_line`).
    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &self,
        window: &mut Window,
        cx: &mut App,
        text: &str,
        face: Face,
        color: Hsla,
        frame: Bounds<Pixels>,
        max_width: f32,
    ) {
        let shown = self.rect(frame);
        draw_line(
            window,
            cx,
            text,
            Face {
                size: self.length(face.size),
                ..face
            },
            self.color(color),
            f32::from(shown.origin.x),
            f32::from(shown.origin.y),
            f32::from(shown.size.height),
            self.length(max_width),
            None,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pose_grows_about_its_origin_and_slides_by_what_is_left_of_the_slide() {
        let frame = NrMotionFrame {
            opacity: 0.5,
            scale: 0.5,
            slide: 0.25,
            done: false,
        };
        let pose = Pose::of(
            &frame,
            point(px(100.0), px(50.0)),
            point(px(0.0), px(-8.0)),
            point(px(20.0), px(0.0)),
        );
        // A quarter of the slide remains: 2 of the 8 pt.
        assert_eq!(pose.at, point(px(100.0), px(48.0)));
        // The origin stays put; other points move halfway towards it.
        assert_eq!(pose.place(20.0, 0.0), point(px(120.0), px(48.0)));
        assert_eq!(pose.place(40.0, 10.0), point(px(130.0), px(53.0)));
        assert_eq!(pose.length(12.0), 6.0);
        assert_eq!(pose.color(gpui::black()).a, 0.5);
    }
}
