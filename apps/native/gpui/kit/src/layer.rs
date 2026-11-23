//! Layered drawing: parts faded as one group (`group`), as AppKit fades transparency layers and
//! menu and tooltip windows, and captures of what lies under a view (`ask_under`, `under`) for
//! blurring, as the Dock does.
//!
//! GPUI draws a group into its own texture and draws that at the group's opacity
//! (`Window::paint_with_opacity`, patched in; see `patches/gpui/UPSTREAM.md`). A glyph inside a
//! fading panel then shows the panel through it rather than what is under the panel, matching
//! AppKit's `a·G + (1 − a·α)·B`, where `G` is the group alone, `α` its coverage and `B` the
//! backdrop.
//!
//! A capture under a view comes from the window's own frame (`render_to_image`) between frames:
//! the frame is drawn again without the view and overlays, read back, then redrawn as shown. It
//! is requested during a frame and taken after it (`Window::defer`), so it works even when the
//! display isn't requesting frames (test windows are often covered). The next frame doesn't
//! request another.

use gpui::{App, Bounds, Pixels, RenderImage, Window, point, px, size};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

/// What the current frame is drawn for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Taking {
    /// The window as shown.
    Nothing,
    /// A capture of what is under a view. The requesting view and overlays draw nothing.
    Under,
}

/// A capture of what was under a view: the device pixels covered (in window points), as
/// straight-alpha RGBA in the window's colour space.
pub struct Under {
    pub pixels: Bounds<Pixels>,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Capture count, so a view can tell when a new one arrives.
    pub taken: u64,
}

thread_local! {
    static TAKING: Cell<Taking> = const { Cell::new(Taking::Nothing) };
    /// The bounds to capture once the current frame finishes.
    static ASKED: Cell<Option<Bounds<Pixels>>> = const { Cell::new(None) };
    /// Set for the frame after a capture, which must not request another.
    static AFTER: Cell<bool> = const { Cell::new(false) };
    /// GPUI can't read frames back here: always on Linux, elsewhere after a failed try.
    static UNREADABLE: Cell<bool> = const { Cell::new(cfg!(target_os = "linux")) };
    static UNDER: RefCell<Option<Rc<Under>>> = const { RefCell::new(None) };
    static UNDER_TAKEN: Cell<u64> = const { Cell::new(0) };
}

pub fn taking() -> Taking {
    TAKING.with(Cell::get)
}

/// Draws a group at `opacity` as one layer, as AppKit fades a view or transparency layer.
/// `draw` paints the parts at full opacity.
pub fn group(
    window: &mut Window,
    cx: &mut App,
    opacity: f32,
    draw: impl FnOnce(&mut Window, &mut App),
) {
    if opacity <= 0.0 {
        return;
    }
    if opacity >= 1.0 {
        return draw(window, cx);
    }
    window.paint_with_opacity(opacity, |window| draw(window, cx));
}

/// Requests a capture of what is under `bounds` (window points) after this frame, leaving out
/// the requesting view and overlays (`Taking::Under`); `under` then returns it. Returns false if
/// no capture will be taken: frames can't be read back here, or this frame follows a capture
/// and the request must wait for the next.
pub fn ask_under(window: &mut Window, cx: &mut App, bounds: Bounds<Pixels>) -> bool {
    if UNREADABLE.with(Cell::get) || AFTER.with(Cell::get) {
        return false;
    }
    if ASKED.with(|asked| asked.replace(Some(bounds))).is_none() {
        window.defer(cx, take);
    }
    true
}

/// The latest capture from `ask_under`, if any.
pub fn under() -> Option<Rc<Under>> {
    UNDER.with(|under| under.borrow().clone())
}

/// Whether window captures work on this platform (known after the first try).
pub fn readable() -> bool {
    !UNREADABLE.with(Cell::get)
}

/// Draws the frame for the capture, reads it back, keeps the requested part, then redraws the
/// frame as shown.
fn take(window: &mut Window, cx: &mut App) {
    let Some(bounds) = ASKED.with(Cell::take) else {
        return;
    };
    TAKING.with(|now| now.set(Taking::Under));
    window.refresh();
    window.draw(cx).clear(cx);
    TAKING.with(|now| now.set(Taking::Nothing));
    match window.render_to_image() {
        Ok(image) => keep_under(window, &image, bounds),
        Err(_) => UNREADABLE.with(|unreadable| unreadable.set(true)),
    }
    AFTER.with(|after| after.set(true));
    window.refresh();
    window.draw(cx).clear(cx);
    AFTER.with(|after| after.set(false));
}

/// The device-pixel rect covering `bounds` (window points) inside a `width` × `height` image at
/// `scale` pixels per point, as first column, first row and counts; `None` if empty.
fn pixels_around(
    bounds: Bounds<Pixels>,
    scale: f32,
    width: u32,
    height: u32,
) -> Option<(u32, u32, u32, u32)> {
    let left = (f32::from(bounds.origin.x) * scale).floor().max(0.0) as u32;
    let top = (f32::from(bounds.origin.y) * scale).floor().max(0.0) as u32;
    let right = ((f32::from(bounds.origin.x + bounds.size.width) * scale).ceil() as u32).min(width);
    let bottom =
        ((f32::from(bounds.origin.y + bounds.size.height) * scale).ceil() as u32).min(height);
    (right > left && bottom > top).then(|| (left, top, right - left, bottom - top))
}

fn points_of(left: u32, top: u32, width: u32, height: u32, scale: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(left as f32 / scale), px(top as f32 / scale)),
        size: size(px(width as f32 / scale), px(height as f32 / scale)),
    }
}

/// Stores the part of `image` under `bounds`.
fn keep_under(window: &Window, image: &image::RgbaImage, bounds: Bounds<Pixels>) {
    let scale = window.scale_factor();
    let Some((left, top, width, height)) =
        pixels_around(bounds, scale, image.width(), image.height())
    else {
        return;
    };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in top..top + height {
        let start = ((y * image.width() + left) * 4) as usize;
        rgba.extend_from_slice(&image.as_raw()[start..start + (width * 4) as usize]);
    }
    let taken = UNDER_TAKEN.with(|count| {
        count.set(count.get() + 1);
        count.get()
    });
    let picture = Under {
        pixels: points_of(left, top, width, height, scale),
        width,
        height,
        rgba,
        taken,
    };
    UNDER.with(|under| *under.borrow_mut() = Some(Rc::new(picture)));
}

impl Under {
    /// The capture blurred like Core Image's Gaussian blur: `sigma` pt, in linear light, with
    /// edges extended. It is blurred at quarter resolution (each pixel averaging 4 × 4) and
    /// scaled back up, which looks the same at this width.
    pub fn blurred(&self, sigma: f32, scale: f32) -> Option<Arc<RenderImage>> {
        let (width, height) = (self.width.div_ceil(COARSER), self.height.div_ceil(COARSER));
        if width == 0 || height == 0 {
            return None;
        }
        let light = self.coarse_light(width, height);
        let kernel = Kernel::new(sigma * scale / COARSER as f32, width, height);
        let blurred = kernel.pass(&kernel.pass(&light, Axis::Across), Axis::Down);
        let buffer = image::ImageBuffer::from_raw(width, height, bgra_of(blurred))?;
        Some(Arc::new(RenderImage::new([image::Frame::new(buffer)])))
    }

    /// The capture downsampled to `width` × `height`, each pixel the mean linear light of those
    /// under it.
    fn coarse_light(&self, width: u32, height: u32) -> Vec<[f32; 3]> {
        let linear: Vec<f32> = (0..=255u8)
            .map(|byte| decode(f32::from(byte) / 255.0))
            .collect();
        let mut light = vec![[0.0f32; 3]; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                light[(y * width + x) as usize] = self.mean_light(x, y, &linear);
            }
        }
        light
    }

    /// The mean linear light of the full-resolution pixels under the coarse pixel at `x`, `y`,
    /// using the `linear` lookup table.
    #[inline]
    fn mean_light(&self, x: u32, y: u32, linear: &[f32]) -> [f32; 3] {
        let (mut sum, mut count) = ([0.0f32; 3], 0.0f32);
        for fine_y in y * COARSER..((y + 1) * COARSER).min(self.height) {
            for fine_x in x * COARSER..((x + 1) * COARSER).min(self.width) {
                let at = ((fine_y * self.width + fine_x) * 4) as usize;
                for (channel, total) in sum.iter_mut().enumerate() {
                    *total += linear[usize::from(self.rgba[at + channel])];
                }
                count += 1.0;
            }
        }
        sum.map(|total| total / count)
    }
}

/// Downsampling factor for the blur, per axis.
const COARSER: u32 = 4;

/// Direction of a blur pass.
#[derive(Clone, Copy)]
enum Axis {
    /// Along each row.
    Across,
    /// Down each column.
    Down,
}

/// Gaussian weights `reach` steps either side, for a `width` × `height` image with extended
/// edges.
struct Kernel {
    weights: Vec<f32>,
    reach: i64,
    width: i64,
    height: i64,
}

impl Kernel {
    /// Normalized weights for a Gaussian of `spread` pixels, out to three times that.
    fn new(spread: f32, width: u32, height: u32) -> Self {
        let reach = (spread * 3.0).ceil() as i64;
        let weights: Vec<f32> = (-reach..=reach)
            .map(|step| (-(step * step) as f32 / (2.0 * spread * spread)).exp())
            .collect();
        let total: f32 = weights.iter().sum();
        Self {
            weights: weights.iter().map(|weight| weight / total).collect(),
            reach,
            width: i64::from(width),
            height: i64::from(height),
        }
    }

    /// `from` blurred along `axis`.
    fn pass(&self, from: &[[f32; 3]], axis: Axis) -> Vec<[f32; 3]> {
        let mut to = vec![[0.0f32; 3]; from.len()];
        for y in 0..self.height {
            for x in 0..self.width {
                to[(y * self.width + x) as usize] = self.blurred_at(from, x, y, axis);
            }
        }
        to
    }

    /// The pixel at `x`, `y` of `from` blurred along one axis.
    #[inline]
    fn blurred_at(&self, from: &[[f32; 3]], x: i64, y: i64, axis: Axis) -> [f32; 3] {
        let mut sum = [0.0f32; 3];
        for (index, weight) in self.weights.iter().enumerate() {
            let (sx, sy) = self.neighbour(x, y, index as i64 - self.reach, axis);
            let value = from[(sy * self.width + sx) as usize];
            for channel in 0..3 {
                sum[channel] += value[channel] * weight;
            }
        }
        sum
    }

    /// The pixel `step` from `x`, `y` along `axis`, clamped to the edge.
    #[inline]
    fn neighbour(&self, x: i64, y: i64, step: i64, axis: Axis) -> (i64, i64) {
        match axis {
            Axis::Across => ((x + step).clamp(0, self.width - 1), y),
            Axis::Down => (x, (y + step).clamp(0, self.height - 1)),
        }
    }
}

/// Linear light back to encoded, opaque BGRA.
fn bgra_of(light: Vec<[f32; 3]>) -> Vec<u8> {
    let byte = |value: f32| (encode(value) * 255.0).round() as u8;
    let mut bgra = Vec::with_capacity(light.len() * 4);
    for [red, green, blue] in light {
        bgra.extend_from_slice(&[byte(blue), byte(green), byte(red), 255]);
    }
    bgra
}

/// The sRGB transfer curve, which Display P3 shares: encoded value to linear light, and back.
fn decode(value: f32) -> f32 {
    if value <= 0.040_45 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}
