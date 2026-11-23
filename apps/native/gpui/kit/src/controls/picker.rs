//! A colour's swatch and the picker it opens.
//!
//! The swatch shows the hex in muted monospace, then a 96 × 28 button with the colour on the
//! left and a caret on the right; its border darkens on hover. A press emits `OpenPicker`, and
//! the core positions the picker (`nr_picker_place`).
//!
//! The picker is a popover with the saturation/brightness square, eyedropper, current colour,
//! hue bar, HEX/RGB mode and fields, a divider and presets. The core owns its state, layout
//! and input (`colour_picker::PickerView` through `nr_picker_*`). The eyedropper emits
//! `Eyedropper`; the popover calls `platform::sample_colour` and feeds the result to `sampled`.

use super::input::{Align, Changed as Typed, Committed, Field, Ground, Placeholder};
use super::{Drawn, Frame};
use super::{Rect, placed};
use crate::core::{c, frame, inset, text};
use crate::draw::{fill, oval, rounded, stroke};
use crate::icons::{self, Glyph};
use crate::nr::{
    NrPickerAct, NrPickerPress, nr_hex_color, nr_picker_act, nr_picker_drag, nr_picker_hover,
    nr_picker_leave, nr_picker_open, nr_picker_press, nr_picker_release, nr_picker_typed_hex,
    nr_picker_view, nr_swatch_hex, picker_layout,
};
use crate::text::{draw_line, width};
use crate::theme::{Face, Token, color, mixed, named};
use gpui::{
    App, AppContext, Background, Bounds, ContentMask, Context, Corners, DispatchPhase, Entity,
    EventEmitter, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, canvas, linear_color_stop, linear_gradient, px,
};
use northroom_core::colour_picker::Layout;

// MARK: The swatch

/// Requests the picker for colour `hex`, next to `swatch` (window points).
pub struct OpenPicker {
    pub swatch: Bounds<Pixels>,
    pub hex: String,
}

fn mono() -> Face {
    Face::mono(12.0)
}

#[derive(Clone)]
pub struct Swatch {
    /// `#` and six upper-case hex digits (`nr_swatch_hex`).
    pub hex: String,
    pub frame: Bounds<Pixels>,
    pub hovered: bool,
}

impl EventEmitter<OpenPicker> for Swatch {}

impl Swatch {
    pub fn new(hex: &str) -> Self {
        let mut swatch = Self {
            hex: String::new(),
            frame: Bounds::default(),
            hovered: false,
        };
        swatch.set(hex);
        swatch
    }

    /// Sets the colour from the picker.
    pub fn set(&mut self, hex: &str) {
        let hex = c(hex);
        // SAFETY: a NUL-terminated colour.
        self.hex = text(&unsafe { nr_swatch_hex(hex.as_ptr()) });
    }

    pub fn size(&self) -> (f32, f32) {
        (width(&self.hex, mono()).ceil() + 8.0 + 96.0, 28.0)
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        cx.notify();
    }

    /// The button's frame in the window.
    pub fn button(&self) -> Bounds<Pixels> {
        let (x, y) = super::origin_of(self.frame);
        frame(x + f32::from(self.frame.size.width) - 96.0, y, 96.0, 28.0)
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App) {
        let (x, y) = super::origin_of(self.frame);
        let w = f32::from(self.frame.size.width);
        draw_line(
            window,
            cx,
            &self.hex,
            mono(),
            color(Token::MutedForeground),
            x,
            y + 6.0,
            16.0,
            w - 104.0,
            None,
        );
        let button = self.button();
        // The colour fills the button inside its 1 px border, leaving 28 px for the caret.
        let (left, top) = super::origin_of(button);
        window.paint_quad(
            gpui::fill(
                frame(left + 1.0, top + 1.0, 96.0 - 2.0 - 28.0, 26.0),
                named(&self.hex),
            )
            .corner_radii(super::left_corners(5.0)),
        );
        let edge = if self.hovered {
            mixed(Token::Foreground, 0.2)
        } else {
            color(Token::Border)
        };
        stroke(window, inset(button, 0.5, 0.5), edge, 5.5, 1.0);
        icons::draw(
            window,
            cx,
            Glyph::CaretDown,
            frame(left + 96.0 - 1.0 - 8.0 - 12.0, top + 8.0, 12.0, 12.0),
            color(Token::MutedForeground),
        );
    }
}

impl Render for Swatch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.clone();
        placed("swatch", cx.entity_id(), self.frame)
            .on_mouse_move(cx.listener(|swatch, event: &MouseMoveEvent, _, cx| {
                let over = crate::core::contains(&swatch.button(), event.position);
                if over != swatch.hovered {
                    swatch.hovered = over;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|swatch, hovered: &bool, _, cx| {
                if !hovered {
                    swatch.hovered = false;
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|swatch, event: &MouseDownEvent, _, cx| {
                    if crate::core::contains(&swatch.button(), event.position) {
                        cx.emit(OpenPicker {
                            swatch: swatch.button(),
                            hex: swatch.hex.clone(),
                        });
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| shown.paint(window, cx),
                )
                .size_full(),
            )
    }
}

// MARK: The picker

/// The picker's colour changed, as six upper-case hex digits.
pub struct ColourChanged(pub String);
/// The eyedropper was pressed. The platform samples a colour and passes it to `sampled`.
pub struct Eyedropper;

/// The open picker's layout (`picker_layout`), relative to its top-left.
fn read_layout() -> Layout {
    picker_layout().expect("the picker is open while it is shown")
}

/// Converts a core sRGB colour to the window's colour space (`crate::display`).
fn hsla(color: northroom_core::ffi::NrColor) -> Hsla {
    crate::display::shown(Rgba {
        r: color.red,
        g: color.green,
        b: color.blue,
        a: color.alpha,
    })
}

/// Parses `hex`, with or without `#` (`nr_hex_color`).
fn hex_colour(hex: &str) -> Hsla {
    let hex = c(hex);
    // SAFETY: a NUL-terminated colour.
    hsla(unsafe { nr_hex_color(hex.as_ptr()) })
}

fn white(alpha: f32) -> Hsla {
    Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: alpha,
    }
    .into()
}

fn black(alpha: f32) -> Hsla {
    Rgba {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: alpha,
    }
    .into()
}

pub struct Picker {
    pub frame: Bounds<Pixels>,
    placed: Layout,
    hex_field: Entity<Field>,
    channel_fields: Vec<Entity<Field>>,
    dragging: bool,
}

impl EventEmitter<ColourChanged> for Picker {}
impl EventEmitter<Eyedropper> for Picker {}

impl Picker {
    /// Opens the picker on `hex` (`nr_picker_open`), showing the hex.
    pub fn new(hex: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let named = c(hex);
        // SAFETY: a NUL-terminated colour.
        unsafe { nr_picker_open(named.as_ptr()) };
        let placed = read_layout();
        let field = |label: &str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut field = Field::new(label, "", window, cx);
                field.face = mono();
                field.align = Align::Centre;
                field.ground = Ground::Bare;
                field.placeholder = Placeholder::Nothing;
                field
            })
        };
        let hex_field = field("Hex", window, cx);
        cx.subscribe_in(&hex_field, window, |picker, field, typed: &Typed, _, cx| {
            // The core decides what the hex field shows while typing.
            let shown = c(&typed.0);
            // SAFETY: a NUL-terminated string.
            let shown = text(&unsafe { nr_picker_typed_hex(shown.as_ptr()) });
            field.update(cx, |field, cx| field.set_content(&shown, cx));
            picker.placed = read_layout();
        })
        .detach();
        cx.subscribe_in(&hex_field, window, |picker, _, left: &Committed, _, cx| {
            // Accept six full hex digits; otherwise restore the current colour.
            if !picker.act(NrPickerAct::LeftHex, 0.0, &left.0, cx) {
                picker.show_fields(None, cx);
            }
        })
        .detach();
        let channel_fields: Vec<Entity<Field>> = ["Red", "Green", "Blue"]
            .into_iter()
            .map(|label| field(label, window, cx))
            .collect();
        for (index, channel) in channel_fields.iter().enumerate() {
            // Each keystroke in a channel field applies immediately when it is a number.
            cx.subscribe_in(channel, window, move |picker, _, typed: &Typed, _, cx| {
                picker.act(NrPickerAct::Typed, index as f64, &typed.0, cx);
            })
            .detach();
            // On commit, restore a channel field that does not hold a number.
            cx.subscribe_in(channel, window, |picker, _, _: &Committed, _, cx| {
                picker.show_fields(None, cx)
            })
            .detach();
        }
        let mut picker = Self {
            frame: Bounds::default(),
            placed,
            hex_field,
            channel_fields,
            dragging: false,
        };
        picker.show_fields(None, cx);
        picker
    }

    pub fn size(&self) -> (f32, f32) {
        (self.placed.width as f32, self.placed.height as f32)
    }

    pub fn layout(&self) -> &Layout {
        &self.placed
    }

    /// Six upper-case hex digits.
    pub fn hex(&self) -> String {
        text(&nr_picker_view().hex)
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        self.frame = frame;
        self.place_fields(cx);
        cx.notify();
    }

    fn place_fields(&mut self, cx: &mut Context<Self>) {
        let origin = self.frame.origin;
        let hex = self.placed.hex_field.at(origin);
        let channels: Vec<Bounds<Pixels>> = self
            .placed
            .channel_fields
            .iter()
            .map(|field| field.at(origin))
            .collect();
        // Hide the other mode's fields with a zero frame.
        let shown = |is: bool, at: Bounds<Pixels>| if is { at } else { Bounds::default() };
        let channels_shown = self.placed.channels;
        self.hex_field.update(cx, |field, cx| {
            let at = shown(!channels_shown, hex);
            field.place(at, at, cx)
        });
        for (field, at) in self.channel_fields.iter().zip(channels) {
            field.update(cx, |field, cx| {
                let at = shown(channels_shown, at);
                field.place(at, at, cx)
            });
        }
    }

    /// Writes the colour into the current mode's fields, leaving field `typing` as typed.
    fn show_fields(&mut self, typing: Option<usize>, cx: &mut Context<Self>) {
        self.placed = read_layout();
        let view = nr_picker_view();
        let hex = text(&view.hex);
        self.hex_field
            .update(cx, |field, cx| field.set_content(&hex, cx));
        for (index, (field, value)) in self
            .channel_fields
            .iter()
            .zip([view.red, view.green, view.blue])
            .enumerate()
        {
            if Some(index) != typing {
                field.update(cx, |field, cx| field.set_content(&value.to_string(), cx));
            }
        }
        self.place_fields(cx);
        cx.notify();
    }

    /// After a colour change: redraws, updates the fields and emits `ColourChanged`.
    fn changed(&mut self, typing: Option<usize>, cx: &mut Context<Self>) {
        self.show_fields(typing, cx);
        cx.emit(ColourChanged(self.hex()));
    }

    /// Sends typed or sampled input to the core (`nr_picker_act`). True when the colour changed.
    fn act(&mut self, action: NrPickerAct, x: f64, words: &str, cx: &mut Context<Self>) -> bool {
        let words = c(words);
        // SAFETY: a NUL-terminated string.
        let changed = unsafe { nr_picker_act(action, x, 0.0, 0.0, words.as_ptr()) };
        if changed {
            let typing = (action == NrPickerAct::Typed).then_some(x as usize);
            self.changed(typing, cx);
        }
        changed
    }

    /// Applies a colour from the eyedropper, channels 0 to 1.
    pub fn sampled(&mut self, red: f64, green: f64, blue: f64, cx: &mut Context<Self>) {
        let none = c("");
        // SAFETY: a NUL-terminated string.
        if unsafe { nr_picker_act(NrPickerAct::Eyedropper, red, green, blue, none.as_ptr()) } {
            self.changed(None, cx);
        }
    }

    fn local(&self, at: Point<Pixels>) -> (f64, f64) {
        (
            f64::from(at.x - self.frame.origin.x),
            f64::from(at.y - self.frame.origin.y),
        )
    }

    /// Whether `at` (window points) is on a visible field, which handles its own presses.
    fn on_field(&self, at: Point<Pixels>) -> bool {
        let origin = self.frame.origin;
        if self.placed.channels {
            self.placed
                .channel_fields
                .iter()
                .any(|field| crate::core::contains(&field.at(origin), at))
        } else {
            crate::core::contains(&self.placed.hex_field.at(origin), at)
        }
    }

    /// Handles a press at `at` (window points).
    pub fn press(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_field(at) {
            return;
        }
        // Blur any field so it commits its text.
        window.blur(cx);
        let (x, y) = self.local(at);
        match nr_picker_press(x, y) {
            NrPickerPress::Changed => {
                self.dragging = true;
                self.changed(None, cx);
            }
            NrPickerPress::Eyedropper => cx.emit(Eyedropper),
            NrPickerPress::ModeTurned => self.show_fields(None, cx),
            NrPickerPress::Nothing => {}
        }
    }

    fn drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let (x, y) = self.local(at);
        if nr_picker_drag(x, y) {
            self.changed(None, cx);
        }
    }

    fn hover(&mut self, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let moved = match at {
            Some(at) => {
                let (x, y) = self.local(at);
                nr_picker_hover(x, y)
            }
            None => nr_picker_leave(),
        };
        if moved {
            self.placed = read_layout();
            cx.notify();
        }
    }

    pub fn paint(&self, window: &mut Window, cx: &mut App) {
        paint(window, cx, self.frame, &self.placed);
    }
}

/// How many pieces each pair of gradient stops is split into (`across`). GPUI on Linux
/// re-encodes gradient ends as sRGB before mixing, so halfway from white to blue lands at
/// 55/255 instead of 128. Over 1/64 of a pair the error stays within one level.
const GRADIENT_PIECES: usize = if crate::display::DISPLAY_P3 {
    16
} else if cfg!(target_os = "linux") {
    64
} else {
    1
};

/// Draws a horizontal gradient across `bounds` through `stops` (position 0 to 1, sRGB colour)
/// the way an sRGB `CGGradient` renders: mixed in sRGB, then converted to the window's colour
/// space. GPUI mixes the raw end colours, so in another colour space each pair of stops is drawn
/// as short pieces between sRGB-mixed colours, keeping the error within one level. Each piece
/// overlaps the next by 1 px to hide seams; the outer ends are rounded by `radius`.
fn across(window: &mut Window, bounds: Bounds<Pixels>, stops: &[(f32, [f32; 3])], radius: f32) {
    let (left, top) = super::origin_of(bounds);
    let (wide, tall) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let shown = |rgb: [f32; 3]| -> Hsla {
        let [r, g, b] = rgb;
        crate::display::shown(Rgba { r, g, b, a: 1.0 })
    };
    let round = |end: bool| px(if end { radius } else { 0.0 });
    let pieces = gradient_pieces(stops);
    for (index, (from, to, start, end)) in pieces.iter().enumerate() {
        let (first, last) = (index == 0, index + 1 == pieces.len());
        let reach = if last { 0.0 } else { 1.0 };
        let corners = Corners {
            top_left: round(first),
            bottom_left: round(first),
            top_right: round(last),
            bottom_right: round(last),
        };
        let (x0, x1) = (left + wide * from, left + wide * to);
        let carried = carried_share(x1 - x0, reach);
        let reached = [0, 1, 2].map(|k| (start[k] + (end[k] - start[k]) * carried).clamp(0.0, 1.0));
        let shade: Background = linear_gradient(
            90.0,
            linear_color_stop(shown(*start), 0.0),
            linear_color_stop(shown(reached), 1.0),
        );
        window.paint_quad(
            gpui::fill(frame(x0, top, x1 - x0 + reach, tall), shade).corner_radii(corners),
        );
    }
}

/// Splits each pair of `stops` into pieces: start and end positions (0 to 1) and their sRGB
/// colours.
fn gradient_pieces(stops: &[(f32, [f32; 3])]) -> Vec<(f32, f32, [f32; 3], [f32; 3])> {
    let mut pieces = Vec::new();
    for pair in stops.windows(2) {
        let ((from, start), (to, end)) = (pair[0], pair[1]);
        for piece in 0..GRADIENT_PIECES {
            let share = |piece: usize| piece as f32 / GRADIENT_PIECES as f32;
            let mix = |share: f32| [0, 1, 2].map(|k| start[k] + (end[k] - start[k]) * share);
            pieces.push((
                from + (to - from) * share(piece),
                from + (to - from) * share(piece + 1),
                mix(share(piece)),
                mix(share(piece + 1)),
            ));
        }
    }
    pieces
}

/// How far through its colour range a piece `width` wide, overlapping the next by `reach`,
/// should be at its right edge. On Linux, with 64 thin pieces per pair, the mix extends under
/// the next piece so the visible part keeps its colours. Elsewhere a piece ends at its own stop.
fn carried_share(width: f32, reach: f32) -> f32 {
    if cfg!(target_os = "linux") {
        (width + reach) / width
    } else {
        1.0
    }
}

/// Tracks a drag on the square or hue bar anywhere in the window until release.
fn follow_drag(window: &mut Window, picker: &Entity<Picker>) {
    let moved = picker.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            moved.update(cx, |picker, cx| picker.drag(event.position, cx));
        }
    });
    let ended = picker.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            ended.update(cx, |picker, cx| {
                picker.dragging = false;
                nr_picker_release();
                cx.notify();
            });
        }
    });
}

/// Draws the picker at `bounds` from the core's layout, except its fields.
fn paint(window: &mut Window, cx: &mut App, bounds: Bounds<Pixels>, placed: &Layout) {
    let origin = bounds.origin;
    let at = |rect: &Rect| rect.at(origin);
    let border = color(Token::Border);
    // Popover surface and border.
    let surface = inset(bounds, 0.5, 0.5);
    rounded(window, surface, color(Token::Popover), 9.5);
    stroke(window, surface, border, 9.5, 1.0);
    let view = nr_picker_view();
    let hue = [view.pure.red, view.pure.green, view.pure.blue];
    let (own, pure) = (hsla(view.color), hsla(view.pure));

    // White to the hue horizontally, with black rising from the bottom.
    let square = at(&placed.square);
    across(window, square, &[(0.0, [1.0; 3]), (1.0, hue)], 6.0);
    window.paint_quad(
        gpui::fill(
            square,
            linear_gradient(
                180.0,
                linear_color_stop(black(0.0), 0.0),
                linear_color_stop(black(1.0), 1.0),
            ),
        )
        .corner_radii(px(6.0)),
    );
    window.with_content_mask(Some(ContentMask { bounds: square }), |window| {
        knob(window, at(&placed.square_knob.frame), own);
    });

    // The eyedropper, then the current colour.
    stroke(
        window,
        inset(at(&placed.dropper), 0.5, 0.5),
        border,
        5.5,
        1.0,
    );
    icons::draw(
        window,
        cx,
        Glyph::Eyedropper,
        at(&placed.dropper_glyph),
        color(Token::MutedForeground),
    );
    let sample = at(&placed.sample);
    rounded(window, sample, own, 6.0);
    stroke(window, inset(sample, 0.5, 0.5), white(0.1), 5.5, 1.0);

    // The hue bar, through the core's stops.
    let bar = at(&placed.hue_bar);
    let stops: Vec<(f32, [f32; 3])> = placed
        .hue_stops
        .iter()
        .map(|stop| {
            (
                stop.at as f32,
                stop.channels.map(|channel| f32::from(channel) / 255.0),
            )
        })
        .collect();
    across(
        window,
        bar,
        &stops,
        6.0f32.min(f32::from(bar.size.height) / 2.0),
    );
    knob(window, at(&placed.hue_knob.frame), pure);

    // "HEX" or "RGB" beside the fields that hold the colour.
    let mode = inset(at(&placed.mode), 0.5, 0.5);
    stroke(window, mode, border, 5.5, 1.0);
    if placed.mode_hovered {
        rounded(window, mode, mixed(Token::Foreground, 0.08), 5.5);
    }
    placed.mode_words.draw(window, cx, origin, None);
    for ground in &placed.grounds {
        rounded(window, at(ground), color(Token::Field), 6.0);
    }

    // A divider, then the presets, with the chosen one ringed.
    fill(window, at(&placed.rule), border);
    for preset in &placed.presets {
        let swatch = at(&preset.frame);
        rounded(window, swatch, hex_colour(&preset.hex), 6.0);
        if preset.chosen {
            // A 1 px gap in the page colour, then a 1.5 px accent ring.
            stroke(
                window,
                inset(swatch, -1.85, -1.85),
                color(Token::Primary),
                8.0,
                1.65,
            );
        } else {
            stroke(window, inset(swatch, 0.5, 0.5), white(0.05), 5.5, 1.0);
        }
    }
}

/// Draws a round handle in `bounds`: the colour under it, a 2 px white edge and a faint dark
/// outline.
fn knob(window: &mut Window, bounds: Bounds<Pixels>, shade: Hsla) {
    oval(window, inset(bounds, -1.0, -1.0), black(0.3));
    oval(window, bounds, white(1.0));
    oval(window, inset(bounds, 2.0, 2.0), shade);
}

impl Render for Picker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();
        let dragging = self.dragging;
        let mut fields = vec![self.hex_field.clone()];
        fields.extend(self.channel_fields.iter().cloned());
        gpui::div()
            .child(
                placed("picker", cx.entity_id(), self.frame)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|picker, event: &MouseDownEvent, window, cx| {
                            picker.press(event.position, window, cx)
                        }),
                    )
                    .on_mouse_move(cx.listener(|picker, event: &MouseMoveEvent, _, cx| {
                        if !picker.dragging {
                            picker.hover(Some(event.position), cx);
                        }
                    }))
                    .on_hover(cx.listener(|picker, hovered: &bool, _, cx| {
                        if !hovered {
                            picker.hover(None, cx);
                        }
                    }))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_, (), window, cx| {
                                let (bounds, placed) = {
                                    let picker = this.read(cx);
                                    (picker.frame, picker.placed.clone())
                                };
                                paint(window, cx, bounds, &placed);
                                if dragging {
                                    follow_drag(window, &this);
                                }
                            },
                        )
                        .size_full(),
                    ),
            )
            .children(fields)
    }
}

// MARK: The picker open

/// The picker closed by a press elsewhere or Escape.
pub struct Closed;

/// The open picker, placed by the core next to its swatch (`nr_picker_place`) above everything
/// else. It holds focus while open. Escape or a press elsewhere closes it, and that press does
/// not reach anything underneath. Closing commits a hex being typed first.
pub struct Popover {
    pub picker: Entity<Picker>,
    focus: gpui::FocusHandle,
    /// The focus before it opened.
    before: Option<gpui::FocusHandle>,
}

impl EventEmitter<Closed> for Popover {}

impl Popover {
    /// Opens the picker on `hex` for the swatch at `swatch` (window points).
    pub fn open(
        swatch: Bounds<Pixels>,
        hex: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| Picker::new(hex, window, cx));
        // The platform loupe samples the screen and hands the colour to the picker, as
        // `NSColorSampler` does.
        cx.subscribe(&picker, |_, picker, _: &Eyedropper, cx| {
            let picker = picker.downgrade();
            let mut later = cx.to_async();
            crate::platform::sample_colour(move |picked| {
                if let Some([red, green, blue]) = picked {
                    let _ = picker.update(&mut later, |picker, cx| {
                        picker.sampled(red, green, blue, cx)
                    });
                }
            });
        })
        .detach();
        let view = window.viewport_size();
        let tall = f32::from(view.height);
        // The core places it in AppKit coordinates, with y up.
        let up = |at: Bounds<Pixels>| northroom_core::ffi::NrRect {
            x: f64::from(at.origin.x),
            y: f64::from(tall - f32::from(at.origin.y + at.size.height)),
            width: f64::from(at.size.width),
            height: f64::from(at.size.height),
        };
        let whole = frame(0.0, 0.0, f32::from(view.width), tall);
        let placed =
            crate::nr::nr_picker_place(up(swatch), up(whole), f64::from(window.scale_factor()));
        let at = frame(
            placed.x as f32,
            tall - (placed.y + placed.height) as f32,
            placed.width as f32,
            placed.height as f32,
        );
        picker.update(cx, |picker, cx| picker.place(at, cx));
        let focus = cx.focus_handle();
        let before = window.focused(cx);
        window.focus(&focus, cx);
        Self {
            picker,
            focus,
            before,
        }
    }

    /// Closes the popover: commits or drops a hex being typed, restores focus and emits
    /// `Closed`.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.blur(cx);
        if let Some(before) = self.before.take() {
            window.focus(&before, cx);
        }
        cx.emit(Closed);
    }

    fn pressed(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::core::contains(&self.picker.read(cx).frame, at) {
            self.close(window, cx);
            return;
        }
        // Keep focus on the popover after a press no field took, so Escape still works.
        if window.focused(cx).is_none() {
            window.focus(&self.focus, cx);
        }
    }
}

impl Render for Popover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = window.viewport_size();
        let this = cx.entity();
        let layer = gpui::div()
            .id("picker-open")
            .absolute()
            .top_0()
            .left_0()
            .w(view.width)
            .h(view.height)
            .track_focus(&self.focus)
            .capture_key_down(
                cx.listener(|popover, event: &gpui::KeyDownEvent, window, cx| {
                    if crate::keys::press(&event.keystroke).key
                        == northroom_core::ffi::NrKey::Escape
                    {
                        cx.stop_propagation();
                        popover.close(window, cx);
                    }
                }),
            )
            .child(
                canvas(
                    |bounds, window, _| {
                        window.insert_hitbox(bounds, gpui::HitboxBehavior::BlockMouse);
                    },
                    move |_, (), window, _| {
                        let this = this.clone();
                        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                            // Bubble phase runs last, after the picker and its fields.
                            if phase == DispatchPhase::Bubble {
                                cx.stop_propagation();
                                this.update(cx, |popover, cx| {
                                    popover.pressed(event.position, window, cx)
                                });
                            }
                        });
                    },
                )
                // Absolute, so the picker's view lays out from the window origin.
                .absolute()
                .size_full(),
            )
            .child(self.picker.clone());
        crate::popover::layer(layer, crate::popover::Layer::Menu)
    }
}
