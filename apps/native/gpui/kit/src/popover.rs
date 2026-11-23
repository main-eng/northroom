//! What every overlay shares: popover placement (`nr_menu_place`), the layer it is drawn in and
//! the layer order.
//!
//! The core places popovers as AppKit does, with y running up and a popover anchored by its
//! lower-left corner. `place` converts that to window points (y down from the top-left), so
//! components only deal in window points.
//!
//! As in GPUI Kit's popover and dialog, a popover is drawn in a window-sized deferred layer at
//! the window's origin. It paints over everything else, its hitboxes block the pointer from what
//! is under it, and its listeners see pointer events before the views do.

use crate::motion::number;
use gpui::{
    AnyElement, Bounds, IntoElement, ParentElement, Pixels, Point, Styled, Window, anchored,
    deferred, div, point, px,
};
use northroom_core::ffi::{NrMotion, NrRect};
use northroom_core::menus::ffi::nr_menu_place;

/// Layer order, lowest first. Menus sit over notices and dialogs because a dialog's select opens
/// a menu; tooltips sit over everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Dialog = 10,
    Toast = 15,
    Menu = 20,
    Tooltip = 30,
}

/// Draws window-sized `content` over the window's views at `layer`. While capturing what lies
/// under a view (`crate::layer::Taking::Under`) the content is kept but clipped to nothing.
pub fn layer(content: impl IntoElement, layer: Layer) -> AnyElement {
    let content = if crate::layer::taking() == crate::layer::Taking::Under {
        div()
            .size_0()
            .overflow_hidden()
            .child(content)
            .into_any_element()
    } else {
        content.into_any_element()
    };
    deferred(anchored().position(point(px(0.0), px(0.0))).child(content))
        .with_priority(layer as usize)
        .into_any_element()
}

/// Where a popover goes and how it comes in, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    /// Top-left at rest.
    pub at: Point<Pixels>,
    /// The full slide-in offset from its resting place.
    pub slide: Point<Pixels>,
    /// The panel-local point it grows from, measured from its top-left.
    pub origin: Point<Pixels>,
}

/// Places a `width` × `height` popover `gap` away from `anchor` (window points), or hanging
/// `below` its control, inside the window and snapped to device pixels (`nr_menu_place`).
pub fn place(
    anchor: Bounds<Pixels>,
    width: f32,
    height: f32,
    gap: f64,
    below: bool,
    window: &Window,
) -> Placed {
    let view = window.viewport_size();
    let (view_width, view_height) = (f64::from(view.width), f64::from(view.height));
    let placed = nr_menu_place(
        up(anchor, view_height),
        f64::from(width),
        f64::from(height),
        NrRect {
            x: 0.0,
            y: 0.0,
            width: view_width,
            height: view_height,
        },
        gap,
        below,
        f64::from(window.scale_factor()),
    );
    let slide = number(NrMotion::Slide) as f32;
    Placed {
        at: point(
            px(placed.x as f32),
            px((view_height - placed.y) as f32 - height),
        ),
        slide: point(
            px(placed.slide_x as f32 * slide),
            px(-placed.slide_y as f32 * slide),
        ),
        origin: point(
            px(placed.origin_x as f32),
            px(height - placed.origin_y as f32),
        ),
    }
}

/// Converts `frame` (window points, y down) to a core rect with y up in a view `height` tall.
fn up(frame: Bounds<Pixels>, height: f64) -> NrRect {
    NrRect {
        x: f64::from(frame.origin.x),
        y: height - f64::from(frame.origin.y + frame.size.height),
        width: f64::from(frame.size.width),
        height: f64::from(frame.size.height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_turns_over_to_the_cores_y_running_up() {
        let frame = crate::core::frame(10.0, 30.0, 40.0, 20.0);
        let turned = up(frame, 800.0);
        assert_eq!(
            (turned.x, turned.y, turned.width, turned.height),
            (10.0, 750.0, 40.0, 20.0)
        );
    }
}
