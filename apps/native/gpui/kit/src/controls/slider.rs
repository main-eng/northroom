//! The slider and its value: a 6 px track in the field colour, filled in the text colour up to
//! a 12 px round handle, then the value in muted monospace. A press or drag sets the value the
//! core maps from the track position (`nr_slider_value`); the core also formats the number
//! (`nr_slider_words`).

use super::{left_corners, placed};
use crate::core::{frame, text};
use crate::draw::{oval, rounded};
use crate::nr::{nr_slider_value, nr_slider_words};
use crate::text::{draw_line, width};
use crate::theme::{Face, Token, color};
use gpui::{
    App, Bounds, Context, DispatchPhase, EventEmitter, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, Styled, Window, canvas,
};

pub const SIZE: (f32, f32) = (256.0, 20.0);
/// The track's left, top, width and height.
const TRACK: (f32, f32, f32, f32) = (0.0, 7.0, 217.0, 6.0);
const HANDLE: f32 = 12.0;

/// The value changed during a drag, 0 to 100 in whole steps.
pub struct Moved(pub f64);
/// The drag ended.
pub struct Released;

#[derive(Clone)]
pub struct Slider {
    pub value: f64,
    pub frame: Bounds<Pixels>,
    dragging: bool,
}

impl EventEmitter<Moved> for Slider {}
impl EventEmitter<Released> for Slider {}

impl Slider {
    pub fn new(value: f64) -> Self {
        Self {
            value,
            frame: Bounds::default(),
            dragging: false,
        }
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        cx.notify();
    }

    /// Sets the value from the pointer's track position at `at` (window points).
    fn drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let x = f32::from(at.x - self.frame.origin.x);
        let share = (x - TRACK.0 - HANDLE / 2.0) / (TRACK.2 - HANDLE);
        let next = nr_slider_value(f64::from(share));
        if next != self.value {
            self.value = next;
            cx.emit(Moved(next));
            cx.notify();
        }
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App, at: Bounds<Pixels>) {
        let (x, y) = super::origin_of(at);
        let (left, down, wide, tall) = TRACK;
        let track = frame(x + left, y + down, wide, tall);
        // The handle's centre stays half a handle in from each end of the track.
        let centre = x + left + HANDLE / 2.0 + (wide - HANDLE) * self.value as f32 / 100.0;
        rounded(window, track, color(Token::Field), tall / 2.0);
        window.paint_quad(
            gpui::fill(
                frame(x + left, y + down, centre - x - left, tall),
                color(Token::Foreground),
            )
            .corner_radii(left_corners(tall / 2.0)),
        );
        oval(
            window,
            frame(
                centre - HANDLE / 2.0,
                y + down + tall / 2.0 - HANDLE / 2.0,
                HANDLE,
                HANDLE,
            ),
            color(Token::Foreground),
        );
        let face = Face::mono(12.0);
        let words = text(&nr_slider_words(self.value));
        draw_line(
            window,
            cx,
            &words,
            face,
            color(Token::MutedForeground),
            x + f32::from(at.size.width) - width(&words, face),
            y + 2.0,
            16.0,
            40.0,
            None,
        );
    }
}

impl Render for Slider {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.clone();
        let this = cx.entity();
        placed("slider", cx.entity_id(), self.frame)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|slider, event: &MouseDownEvent, _, cx| {
                    slider.dragging = true;
                    slider.drag(event.position, cx);
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    // Paint at the core's frame, not the pixel-rounded layout.
                    move |_, (), window, cx| {
                        shown.paint(window, cx, shown.frame);
                        if shown.dragging {
                            // Track the drag anywhere in the window until release.
                            let moved = this.clone();
                            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    moved.update(cx, |slider, cx| slider.drag(event.position, cx));
                                }
                            });
                            let ended = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase == DispatchPhase::Bubble {
                                    ended.update(cx, |slider, cx| {
                                        slider.dragging = false;
                                        cx.emit(Released);
                                        cx.notify();
                                    });
                                }
                            });
                        }
                    },
                )
                .size_full(),
            )
    }
}
