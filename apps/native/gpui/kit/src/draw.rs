//! The shapes views are drawn with, named after AppKit's calls. A stroke is centred on its path,
//! as `NSBezierPath.stroke` draws it.

use crate::core::inset;
use gpui::{Bounds, Corners, Hsla, Pixels, RenderImage, Window, px, transparent_black};
use std::sync::Arc;

pub fn fill(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(gpui::fill(bounds, color));
}

pub fn rounded(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla, radius: f32) {
    window.paint_quad(gpui::fill(bounds, color).corner_radii(px(radius)));
}

/// A rounded rect's outline, `width` thick and centred on its edge.
pub fn stroke(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla, radius: f32, width: f32) {
    let outer = inset(bounds, -width / 2.0, -width / 2.0);
    window.paint_quad(gpui::quad(
        outer,
        px(radius + width / 2.0),
        transparent_black(),
        px(width),
        color,
        gpui::BorderStyle::Solid,
    ));
}

pub fn oval(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    let radius = f32::from(bounds.size.width.min(bounds.size.height)) / 2.0;
    rounded(window, bounds, color, radius);
}

/// Which part of a picture larger than its frame is kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Crop {
    /// Centred (`object-position: center`), for pictures and video.
    Centre,
    /// Top edge kept, centred across (`object-position: top`), so a document shows its first
    /// lines.
    Top,
}

/// `image` filling `frame`, cropped about its centre (`object-cover`), its corners rounded.
pub fn cover(
    window: &mut Window,
    image: &Arc<RenderImage>,
    frame: Bounds<Pixels>,
    radii: Corners<Pixels>,
) {
    cover_cropped(window, image, frame, radii, Crop::Centre);
}

/// `image` filling `frame` with its top edge kept (`object-cover` at `object-position: top`),
/// its corners rounded, so a document's page shows its first lines.
pub fn cover_from_top(
    window: &mut Window,
    image: &Arc<RenderImage>,
    frame: Bounds<Pixels>,
    radii: Corners<Pixels>,
) {
    cover_cropped(window, image, frame, radii, Crop::Top);
}

/// Draws `image` whole and centred in `frame`, as large as fits (`object-contain`), its corners
/// rounded. Returns where it was drawn.
pub fn contain(
    window: &mut Window,
    image: &Arc<RenderImage>,
    frame: Bounds<Pixels>,
    radii: Corners<Pixels>,
) -> Bounds<Pixels> {
    let size = image.size(0);
    let (width, height) = (size.width.0 as f32, size.height.0 as f32);
    let (frame_width, frame_height) = (f32::from(frame.size.width), f32::from(frame.size.height));
    let scale = (frame_width / width).min(frame_height / height);
    let (shown_width, shown_height) = (width * scale, height * scale);
    let shown = Bounds {
        origin: gpui::point(
            frame.origin.x + px((frame_width - shown_width) / 2.0),
            frame.origin.y + px((frame_height - shown_height) / 2.0),
        ),
        size: gpui::size(px(shown_width), px(shown_height)),
    };
    let _ = window.paint_image(shown, shown, radii, image.clone(), 0, false);
    shown
}

fn cover_cropped(
    window: &mut Window,
    image: &Arc<RenderImage>,
    frame: Bounds<Pixels>,
    radii: Corners<Pixels>,
    crop: Crop,
) {
    let size = image.size(0);
    let (width, height) = (size.width.0 as f32, size.height.0 as f32);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let (frame_width, frame_height) = (f32::from(frame.size.width), f32::from(frame.size.height));
    let scale = (frame_width / width).max(frame_height / height);
    let (shown_width, shown_height) = (width * scale, height * scale);
    let top = match crop {
        Crop::Centre => (frame_height - shown_height) / 2.0,
        Crop::Top => 0.0,
    };
    let image_bounds = Bounds {
        origin: gpui::point(
            frame.origin.x + px((frame_width - shown_width) / 2.0),
            frame.origin.y + px(top),
        ),
        size: gpui::size(px(shown_width), px(shown_height)),
    };
    let _ = window.paint_image(frame, image_bounds, radii, image.clone(), 0, false);
}
