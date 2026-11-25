//! Draws paths, strokes and pictures into an RGBA canvas with anti-aliased edges: a page's
//! picture. Coverage is accumulated as signed area per pixel and summed along each row, so a
//! path's overlapping parts fill once, as the nonzero rule fills them. Strokes become one
//! rectangle per segment and a disc at each joint, filled together.
//!
//! Left out: the even-odd rule (filled as nonzero), dashes (drawn solid), and line caps and
//! joins beyond round joints.

use super::stroke::Pen;
use crate::deck::Command;

/// How far a flattened curve may stray from the true curve, in pixels.
const TOLERANCE: f64 = 0.2;
/// The most line segments one curve is flattened into.
const SEGMENTS_MAX: usize = 256;
/// The narrowest a stroke is drawn, in pixels: thinner lines are drawn this wide and fainter.
const HAIRLINE: f64 = 1.0;

/// An RGBA colour with straight alpha, each from 0 to 1.
pub type Rgba = [f32; 4];

/// A coverage mask over the whole canvas, 0 to 255 a pixel, for clipping.
pub type Mask = Vec<u8>;

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    /// Premultiplied RGBA.
    pixels: Vec<u8>,
}

/// A path flattened into polygons, each a list of points.
type Polygons = Vec<Vec<(f64, f64)>>;

fn flatten_cubic(
    out: &mut Vec<(f64, f64)>,
    from: (f64, f64),
    a: (f64, f64),
    b: (f64, f64),
    to: (f64, f64),
) {
    let deviation = ((from.0 - 2.0 * a.0 + b.0).hypot(from.1 - 2.0 * a.1 + b.1))
        .max((a.0 - 2.0 * b.0 + to.0).hypot(a.1 - 2.0 * b.1 + to.1));
    let count = ((0.75 * deviation / TOLERANCE).sqrt().ceil() as usize).clamp(1, SEGMENTS_MAX);
    for step in 1..=count {
        let t = step as f64 / count as f64;
        let u = 1.0 - t;
        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            w0 * from.0 + w1 * a.0 + w2 * b.0 + w3 * to.0,
            w0 * from.1 + w1 * a.1 + w2 * b.1 + w3 * to.1,
        ));
    }
}

/// A path's polygons: each subpath, curves turned into line segments.
pub fn flatten(path: &[Command]) -> Polygons {
    let mut flat = Flattening::default();
    for command in path {
        flat.take(*command);
    }
    flat.end_outline();
    flat.polygons
}

/// Outlines flattened so far, and the one being flattened.
#[derive(Default)]
struct Flattening {
    polygons: Polygons,
    current: Vec<(f64, f64)>,
    start: (f64, f64),
}

impl Flattening {
    fn end_outline(&mut self) {
        if self.current.len() > 1 {
            self.polygons.push(std::mem::take(&mut self.current));
        }
        self.current.clear();
    }

    fn take(&mut self, command: Command) {
        let last = self.current.last().copied().unwrap_or(self.start);
        match command {
            Command::Move(x, y) => {
                self.end_outline();
                self.start = (x, y);
                self.current.push(self.start);
            }
            Command::Line(x, y) => self.current.push((x, y)),
            Command::Quad(cx, cy, x, y) => {
                let a = (
                    last.0 + 2.0 / 3.0 * (cx - last.0),
                    last.1 + 2.0 / 3.0 * (cy - last.1),
                );
                let b = (x + 2.0 / 3.0 * (cx - x), y + 2.0 / 3.0 * (cy - y));
                flatten_cubic(&mut self.current, last, a, b, (x, y));
            }
            Command::Cubic(ax, ay, bx, by, x, y) => {
                flatten_cubic(&mut self.current, last, (ax, ay), (bx, by), (x, y));
            }
            Command::Close => {
                // The closing side, which a stroke draws and a fill implies.
                if self.current.len() > 1 && last != self.start {
                    self.current.push(self.start);
                }
                self.end_outline();
                self.current.push(self.start);
            }
        }
    }
}

/// Signed area coverage, accumulated per pixel in a box.
struct Accumulator {
    left: i64,
    top: i64,
    width: usize,
    height: usize,
    cells: Vec<f32>,
}

impl Accumulator {
    fn new(left: i64, top: i64, width: usize, height: usize) -> Self {
        Self {
            left,
            top,
            width,
            height,
            cells: vec![0.0; (width + 2) * height],
        }
    }

    /// Adds a line's signed area to the cells it crosses.
    fn line(&mut self, from: (f64, f64), to: (f64, f64)) {
        let stride = self.width + 2;
        let clamp_x = |x: f64| (x - self.left as f64).clamp(0.0, self.width as f64);
        let (mut x0, mut y0) = (clamp_x(from.0), from.1 - self.top as f64);
        let (mut x1, mut y1) = (clamp_x(to.0), to.1 - self.top as f64);
        if (y0 - y1).abs() < f64::EPSILON {
            return;
        }
        let direction: f32 = if y0 < y1 { 1.0 } else { -1.0 };
        if y0 > y1 {
            std::mem::swap(&mut x0, &mut x1);
            std::mem::swap(&mut y0, &mut y1);
        }
        let slope = (x1 - x0) / (y1 - y0);
        let mut x = x0;
        if y0 < 0.0 {
            x -= y0 * slope;
            y0 = 0.0;
        }
        let y1 = y1.min(self.height as f64);
        let mut row = y0.floor().max(0.0) as usize;
        while (row as f64) < y1 && row < self.height {
            let top = (row as f64).max(y0);
            let bottom = (row as f64 + 1.0).min(y1);
            let dy = bottom - top;
            let next = (x + slope * dy).clamp(0.0, self.width as f64);
            self.cross(row * stride, x, next, dy as f32 * direction);
            x = next;
            row += 1;
        }
    }

    /// Adds one row's share of a line from `x` to `next`, `amount` its signed height.
    fn cross(&mut self, start: usize, x: f64, next: f64, amount: f32) {
        let (low, high) = if x < next { (x, next) } else { (next, x) };
        let low_floor = low.floor();
        let first = low_floor as usize;
        let high_ceil = high.ceil();
        let last = high_ceil as usize;
        if last <= first + 1 {
            let middle = (0.5 * (x + next) - low_floor) as f32;
            self.cells[start + first] += amount - amount * middle;
            self.cells[start + first + 1] += amount * middle;
            return;
        }
        let scale = (1.0 / (high - low)) as f32;
        let low_fraction = (low - low_floor) as f32;
        let first_area = 0.5 * scale * (1.0 - low_fraction) * (1.0 - low_fraction);
        let high_fraction = (high - high_ceil + 1.0) as f32;
        let last_area = 0.5 * scale * high_fraction * high_fraction;
        self.cells[start + first] += amount * first_area;
        if last == first + 2 {
            self.cells[start + first + 1] += amount * (1.0 - first_area - last_area);
        } else {
            let second = scale * (1.5 - low_fraction);
            self.cells[start + first + 1] += amount * (second - first_area);
            for column in first + 2..last - 1 {
                self.cells[start + column] += amount * scale;
            }
            let before_last = second + (last - first - 3) as f32 * scale;
            self.cells[start + last - 1] += amount * (1.0 - before_last - last_area);
        }
        self.cells[start + last] += amount * last_area;
    }

    fn polygon(&mut self, points: &[(f64, f64)]) {
        for pair in points.windows(2) {
            self.line(pair[0], pair[1]);
        }
        if let (Some(first), Some(last)) = (points.first(), points.last()) {
            self.line(*last, *first);
        }
    }

    /// Each pixel's coverage, row by row, from 0 to 1.
    fn coverage(&self, mut each: impl FnMut(usize, usize, f32)) {
        let stride = self.width + 2;
        for row in 0..self.height {
            let mut sum = 0.0f32;
            for column in 0..self.width {
                sum += self.cells[row * stride + column];
                let covered = sum.abs().min(1.0);
                if covered > 0.002 {
                    each(column, row, covered);
                }
            }
        }
    }
}

/// The pixel box polygons cover, inside a canvas `width` × `height`.
fn bounds(polygons: &Polygons, width: usize, height: usize) -> Option<(i64, i64, usize, usize)> {
    let points = polygons.iter().flatten();
    let (mut left, mut top, mut right, mut bottom) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in points {
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        left = left.min(x);
        top = top.min(y);
        right = right.max(x);
        bottom = bottom.max(y);
    }
    let left = left.floor().max(0.0) as i64;
    let top = top.floor().max(0.0) as i64;
    let right = right.ceil().min(width as f64) as i64;
    let bottom = bottom.ceil().min(height as f64) as i64;
    (right > left && bottom > top)
        .then(|| (left, top, (right - left) as usize, (bottom - top) as usize))
}

impl Canvas {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![0; width * height * 4],
        }
    }

    fn blend(&mut self, index: usize, colour: Rgba, amount: f32) {
        let alpha = colour[3] * amount;
        if alpha <= 0.0 {
            return;
        }
        let pixel = &mut self.pixels[index * 4..index * 4 + 4];
        let keep = 1.0 - alpha;
        for channel in 0..3 {
            let value = colour[channel] * alpha * 255.0 + f32::from(pixel[channel]) * keep;
            pixel[channel] = value.round().clamp(0.0, 255.0) as u8;
        }
        pixel[3] = (alpha * 255.0 + f32::from(pixel[3]) * keep)
            .round()
            .clamp(0.0, 255.0) as u8;
    }

    fn paint(&mut self, polygons: &Polygons, colour: Rgba, clip: Option<&Mask>) {
        let Some((left, top, width, height)) = bounds(polygons, self.width, self.height) else {
            return;
        };
        let mut accumulator = Accumulator::new(left, top, width, height);
        for polygon in polygons {
            accumulator.polygon(polygon);
        }
        let stride = self.width;
        let mut covered = Vec::new();
        accumulator.coverage(|column, row, amount| covered.push((column, row, amount)));
        for (column, row, amount) in covered {
            let index = (top as usize + row) * stride + left as usize + column;
            let clipped = clip.map_or(1.0, |mask| f32::from(mask[index]) / 255.0);
            self.blend(index, colour, amount * clipped);
        }
    }

    /// Fills a path given in pixels.
    pub fn fill(&mut self, path: &[Command], colour: Rgba, clip: Option<&Mask>) {
        self.paint(&flatten(path), colour, clip);
    }

    /// Strokes a path given in pixels, `width` pixels wide.
    /// Strokes a path given in pixels with `pen`, its lengths in pixels. A line thinner than a
    /// pixel is drawn a pixel wide and fainter.
    pub fn stroke(&mut self, path: &[Command], pen: &Pen, colour: Rgba, clip: Option<&Mask>) {
        let faint = (pen.width / HAIRLINE).clamp(0.0, 1.0) as f32;
        let colour = [colour[0], colour[1], colour[2], colour[3] * faint.max(0.35)];
        let pen = Pen {
            width: pen.width.max(HAIRLINE),
            ..pen.clone()
        };
        let polygons = super::stroke::outline(&flatten(path), &pen);
        self.paint(&polygons, colour, clip);
    }

    /// The coverage of a path given in pixels, as a clip mask within `within`, if any.
    pub fn clip_mask(&self, path: &[Command], within: Option<&Mask>) -> Mask {
        let mut mask = vec![0u8; self.width * self.height];
        let polygons = flatten(path);
        if let Some((left, top, width, height)) = bounds(&polygons, self.width, self.height) {
            let mut accumulator = Accumulator::new(left, top, width, height);
            for polygon in &polygons {
                accumulator.polygon(polygon);
            }
            let stride = self.width;
            accumulator.coverage(|column, row, amount| {
                mask[(top as usize + row) * stride + left as usize + column] =
                    (amount * 255.0) as u8;
            });
        }
        if let Some(within) = within {
            for (value, outer) in mask.iter_mut().zip(within) {
                *value = (u16::from(*value) * u16::from(*outer) / 255) as u8;
            }
        }
        mask
    }

    /// Draws straight-alpha RGBA pixels `width` × `height`, the unit square mapped to pixels by
    /// `matrix` (the image's top row at y = 1), sampled smoothly.
    pub fn image(
        &mut self,
        pixels: &[u8],
        size: (usize, usize),
        matrix: [f64; 6],
        alpha: f32,
        clip: Option<&Mask>,
    ) {
        let (width, height) = size;
        let Some(inverse) = super::render::invert(matrix) else {
            return;
        };
        let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .map(|point| super::render::apply(matrix, point));
        let Some((left, top, box_width, box_height)) =
            bounds(&vec![corners.to_vec()], self.width, self.height)
        else {
            return;
        };
        let sample = |u: f64, v: f64| -> Rgba {
            let x = (u * width as f64 - 0.5).clamp(0.0, width as f64 - 1.0);
            let y = ((1.0 - v) * height as f64 - 0.5).clamp(0.0, height as f64 - 1.0);
            let (x0, y0) = (x.floor() as usize, y.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
            let at = |px: usize, py: usize, channel: usize| {
                f32::from(pixels[(py * width + px) * 4 + channel]) / 255.0
            };
            std::array::from_fn(|channel| {
                let top_row = at(x0, y0, channel) * (1.0 - fx) + at(x1, y0, channel) * fx;
                let bottom_row = at(x0, y1, channel) * (1.0 - fx) + at(x1, y1, channel) * fx;
                top_row * (1.0 - fy) + bottom_row * fy
            })
        };
        for row in top as usize..top as usize + box_height {
            for column in left as usize..left as usize + box_width {
                let (u, v) = super::render::apply(inverse, (column as f64 + 0.5, row as f64 + 0.5));
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let index = row * self.width + column;
                let clipped = clip.map_or(1.0, |mask| f32::from(mask[index]) / 255.0);
                let colour = sample(u, v);
                self.blend(index, colour, alpha * clipped);
            }
        }
    }

    /// Paints each pixel the colour `colour_at` gives for its centre, within the clip.
    pub fn shade(&mut self, colour_at: impl Fn((f64, f64)) -> Option<Rgba>, clip: Option<&Mask>) {
        for row in 0..self.height {
            for column in 0..self.width {
                let index = row * self.width + column;
                let clipped = clip.map_or(1.0, |mask| f32::from(mask[index]) / 255.0);
                if clipped <= 0.0 {
                    continue;
                }
                if let Some(colour) = colour_at((column as f64 + 0.5, row as f64 + 0.5)) {
                    self.blend(index, colour, clipped);
                }
            }
        }
    }

    pub fn is_blank(&self) -> bool {
        self.pixels
            .iter()
            .skip(3)
            .step_by(4)
            .all(|alpha| *alpha == 0)
    }

    /// The canvas as a PNG, alpha straight.
    pub fn png(&self) -> Vec<u8> {
        let mut straight = self.pixels.clone();
        for pixel in straight.as_chunks_mut::<4>().0 {
            let alpha = f32::from(pixel[3]);
            if alpha > 0.0 && alpha < 255.0 {
                for channel in &mut pixel[..3] {
                    *channel = (f32::from(*channel) * 255.0 / alpha).round().min(255.0) as u8;
                }
            }
        }
        crate::deck::picture::encode_png(self.width as u32, self.height as u32, &straight)
    }

    #[cfg(test)]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let at = (y * self.width + x) * 4;
        self.pixels[at..at + 4].try_into().expect("four")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(left: f64, top: f64, size: f64) -> Vec<Command> {
        vec![
            Command::Move(left, top),
            Command::Line(left + size, top),
            Command::Line(left + size, top + size),
            Command::Line(left, top + size),
            Command::Close,
        ]
    }

    #[test]
    fn a_square_fills_its_pixels_and_half_covers_its_edge() {
        let mut canvas = Canvas::new(10, 10);
        canvas.fill(&square(2.0, 2.0, 4.5), [1.0, 0.0, 0.0, 1.0], None);
        assert_eq!(canvas.pixel(3, 3), [255, 0, 0, 255]);
        assert_eq!(canvas.pixel(0, 0), [0, 0, 0, 0]);
        let edge = canvas.pixel(6, 3);
        assert!((120..=135).contains(&edge[3]), "{edge:?}");
        // Overlapping squares fill once.
        let mut overlap = Canvas::new(10, 10);
        let mut both = square(1.0, 1.0, 5.0);
        both.extend(square(3.0, 3.0, 5.0));
        overlap.fill(&both, [0.0, 0.0, 0.0, 0.5], None);
        assert_eq!(overlap.pixel(4, 4)[3], 128);
    }

    #[test]
    fn a_thick_strokes_corners_fill_solid() {
        let mut canvas = Canvas::new(40, 40);
        let square = [
            Command::Move(10.0, 10.0),
            Command::Line(30.0, 10.0),
            Command::Line(30.0, 30.0),
            Command::Line(10.0, 30.0),
            Command::Close,
        ];
        canvas.stroke(&square, &Pen::solid(6.0), [0.0, 0.0, 0.0, 1.0], None);
        for (x, y) in [(10, 10), (30, 10), (30, 30), (10, 30), (31, 9), (9, 31)] {
            assert_eq!(canvas.pixels[(y * 40 + x) * 4 + 3], 255, "({x}, {y})");
        }
    }

    #[test]
    fn a_clip_limits_a_fill_and_a_stroke_covers_its_width() {
        let mut canvas = Canvas::new(10, 10);
        let clip = canvas.clip_mask(&square(0.0, 0.0, 5.0), None);
        canvas.fill(&square(0.0, 0.0, 10.0), [0.0, 0.0, 1.0, 1.0], Some(&clip));
        assert_eq!(canvas.pixel(2, 2)[3], 255);
        assert_eq!(canvas.pixel(7, 7)[3], 0);
        let mut lines = Canvas::new(10, 10);
        lines.stroke(
            &[Command::Move(0.0, 5.0), Command::Line(10.0, 5.0)],
            &Pen::solid(2.0),
            [0.0, 0.0, 0.0, 1.0],
            None,
        );
        assert_eq!(lines.pixel(5, 4)[3], 255);
        assert_eq!(lines.pixel(5, 2)[3], 0);
    }

    #[test]
    fn an_image_maps_its_top_row_to_the_top() {
        let mut canvas = Canvas::new(4, 4);
        // Two pixels tall: red on top, blue below.
        let pixels = [255, 0, 0, 255, 0, 0, 255, 255];
        canvas.image(&pixels, (1, 2), [4.0, 0.0, 0.0, -4.0, 0.0, 4.0], 1.0, None);
        assert_eq!(canvas.pixel(1, 0)[0], 255);
        assert_eq!(canvas.pixel(1, 3)[2], 255);
    }
}
