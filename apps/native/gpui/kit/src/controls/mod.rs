//! Form controls for settings and dialogs: buttons, selects, one-line fields, the slider, the
//! theme choice, the colour swatch and picker, and the spinner. Each is a GPUI view placed at
//! the core's layout frame (`place`) that draws its state and reports input through events.
//! The core owns each control's value, states and input handling. A dialog decides which
//! control shows the focus ring (`nr_dialog_focus`).

pub mod button;
pub mod edit;
pub mod heard;
pub mod input;
pub mod picker;
pub mod select;
pub mod slider;
pub mod spinner;
pub mod theme_choice;

use crate::draw::stroke;
use crate::text::draw_line;
use crate::theme::{self, Face, Token};
use gpui::{App, Bounds, Hsla, Pixels, Rgba, Window, px};

/// Opacity multiplier for a disabled control.
pub const DISABLED: f32 = 0.5;

/// The core's layout types (rects are top-left origin, y down), drawn as given.
pub use northroom_core::board::card::Font;
pub use northroom_core::dialog::layout::{Button as PlacedButton, Line};
pub use northroom_core::geometry::Rect;

/// Converts a core rect to window bounds.
pub trait Frame {
    fn bounds(&self) -> Bounds<Pixels>;
    /// The rect offset by `origin`, from view points to window points.
    fn at(&self, origin: gpui::Point<Pixels>) -> Bounds<Pixels>;
}

impl Frame for Rect {
    fn bounds(&self) -> Bounds<Pixels> {
        crate::core::frame(
            self.x as f32,
            self.y as f32,
            self.width as f32,
            self.height as f32,
        )
    }

    fn at(&self, origin: gpui::Point<Pixels>) -> Bounds<Pixels> {
        crate::core::offset(self.bounds(), f32::from(origin.x), f32::from(origin.y))
    }
}

/// The face a core font names. Monospace has a single weight (`Face::of`).
pub trait Faced {
    fn face(&self) -> Face;
}

impl Faced for Font {
    fn face(&self) -> Face {
        Face {
            size: self.size as f32,
            weight: if self.mono { 400.0 } else { self.weight as f32 },
            mono: self.mono,
            tabular: self.tabular,
            slanted: self.slanted,
        }
    }
}

/// Draws a line of text the core placed, set as a browser would, in its theme colour and
/// opacity.
pub trait Drawn {
    fn ink(&self) -> Hsla;
    /// Draws the line offset by `origin`, in its own colour or `ink`.
    fn draw(
        &self,
        window: &mut Window,
        cx: &mut App,
        origin: gpui::Point<Pixels>,
        ink: Option<Hsla>,
    );
}

impl Drawn for Line {
    fn ink(&self) -> Hsla {
        theme::faded(theme::color(self.color), self.opacity)
    }

    fn draw(
        &self,
        window: &mut Window,
        cx: &mut App,
        origin: gpui::Point<Pixels>,
        ink: Option<Hsla>,
    ) {
        let at = self.frame.at(origin);
        draw_line(
            window,
            cx,
            &self.text,
            self.font.face(),
            ink.unwrap_or_else(|| self.ink()),
            f32::from(at.origin.x),
            f32::from(at.origin.y),
            f32::from(at.size.height),
            f32::from(at.size.width),
            None,
        );
    }
}

/// `color` as drawn by a control: unchanged when enabled, at `DISABLED` opacity otherwise.
pub fn strength(color: Hsla, enabled: bool) -> Hsla {
    if enabled {
        color
    } else {
        color.alpha(color.a * DISABLED)
    }
}

/// Composites `over` onto opaque `under` as one opaque colour, so text on a fill dims with it
/// as a single layer (like a Core Graphics transparency layer). GPUI dims each shape separately.
pub fn flattened(over: Hsla, under: Hsla) -> Hsla {
    let (over, under): (Rgba, Rgba) = (over.into(), under.into());
    let mix = |top: f32, bottom: f32| top * over.a + bottom * (1.0 - over.a);
    Rgba {
        r: mix(over.r, under.r),
        g: mix(over.g, under.g),
        b: mix(over.b, under.b),
        a: 1.0,
    }
    .into()
}

/// The focus ring of a dialog button: 2 px in the ring colour, 1 px off its edge.
pub fn button_ring(window: &mut Window, frame: Bounds<Pixels>) {
    stroke(
        window,
        crate::core::inset(frame, -2.0, -2.0),
        theme::color(Token::Ring),
        8.0,
        2.0,
    );
}

/// The focus ring of a dialog select or focused field: 2 px of the ring colour at 30%, just
/// outside its edge.
pub fn soft_ring(window: &mut Window, frame: Bounds<Pixels>) {
    stroke(
        window,
        crate::core::inset(frame, -1.0, -1.0),
        theme::mixed(Token::Ring, 0.3),
        7.0,
        2.0,
    );
}

/// The frame's origin as `(x, y)`. Zero until the owner places the control.
pub fn origin_of(frame: Bounds<Pixels>) -> (f32, f32) {
    (f32::from(frame.origin.x), f32::from(frame.origin.y))
}

/// Corners rounded on the left side only.
pub fn left_corners(radius: f32) -> gpui::Corners<Pixels> {
    gpui::Corners {
        top_left: px(radius),
        bottom_left: px(radius),
        top_right: px(0.0),
        bottom_right: px(0.0),
    }
}

/// The absolutely positioned element a control draws in, at its window frame.
pub fn placed(
    id: &'static str,
    entity: gpui::EntityId,
    frame: Bounds<Pixels>,
) -> gpui::Stateful<gpui::Div> {
    use gpui::{InteractiveElement, Styled};
    gpui::div()
        .id((id, entity))
        .absolute()
        .left(frame.origin.x)
        .top(frame.origin.y)
        .w(frame.size.width)
        .h(frame.size.height)
}
