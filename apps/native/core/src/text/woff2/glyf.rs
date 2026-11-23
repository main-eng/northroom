//! Reverses WOFF2's `glyf` and `loca` transform (WOFF2 §5.1). The transformed table splits
//! glyph data into separate streams (contour counts, point counts, flags, packed coordinates,
//! composites, boxes, instructions); this writes the glyphs back in TrueType layout and builds
//! `loca` from their offsets.

use super::{u16_at, u32_at};

/// Transformed table header: reserved, options, glyph count, `loca` format, then seven stream
/// lengths; 36 bytes.
const HEADER_LENGTH: usize = 36;
/// Option bit: an overlap bitmap follows.
const HAS_OVERLAP_BITMAP: u16 = 1;

/// The flag on a simple glyph's first point marking overlapping contours (`OVERLAP_SIMPLE`).
const OVERLAP_SIMPLE: u8 = 0x40;
const ON_CURVE: u8 = 0x01;
const X_SHORT: u8 = 0x02;
const Y_SHORT: u8 = 0x04;
/// With a short x, x is positive; with a long x, x repeats the previous value.
const X_SAME_OR_POSITIVE: u8 = 0x10;
const Y_SAME_OR_POSITIVE: u8 = 0x20;

/// TrueType composite component flags.
const ARGS_ARE_WORDS: u16 = 0x0001;
const HAS_SCALE: u16 = 0x0008;
const MORE_COMPONENTS: u16 = 0x0020;
const HAS_X_AND_Y_SCALE: u16 = 0x0040;
const HAS_TWO_BY_TWO: u16 = 0x0080;
const HAS_INSTRUCTIONS: u16 = 0x0100;

/// A forward reader over one stream of the transformed table.
pub(super) struct Stream<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Stream<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, at: 0 }
    }

    fn take(&mut self, length: usize) -> &'a [u8] {
        let taken = &self.data[self.at..self.at + length];
        self.at += length;
        taken
    }

    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    fn u16(&mut self) -> u16 {
        u16::from_be_bytes([self.u8(), self.u8()])
    }

    fn i16(&mut self) -> i16 {
        self.u16() as i16
    }

    /// Reads a 255UInt16: one byte; a word after 253; or the next byte plus 253 after 255, or
    /// plus 506 after 254.
    pub(super) fn u255(&mut self) -> usize {
        match self.u8() {
            253 => usize::from(self.u16()),
            255 => 253 + usize::from(self.u8()),
            254 => 506 + usize::from(self.u8()),
            code => usize::from(code),
        }
    }
}

/// Whether glyph `index`'s bit is set in `bitmap` (MSB first).
fn bit(bitmap: &[u8], index: usize) -> bool {
    bitmap[index >> 3] & (0x80 >> (index & 7)) != 0
}

/// A simple glyph's point, in font units.
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
    on_curve: bool,
}

/// `base` signed by `flag`'s low bit; set means positive.
fn signed(flag: u8, base: i32) -> i32 {
    if flag & 1 == 1 {
        base
    } else {
        -base
    }
}

/// Decodes a point delta from a WOFF2 triplet (§5.2). The flag's low 7 bits give how many bytes
/// of `glyphs` follow and how they split between x and y.
fn triplet(flag: u8, glyphs: &mut Stream) -> (i32, i32) {
    let flag = flag & 0x7f;
    let byte = |glyphs: &mut Stream| i32::from(glyphs.u8());
    match flag {
        0..10 => (0, signed(flag, (i32::from(flag & 14) << 7) + byte(glyphs))),
        10..20 => (
            signed(flag, (i32::from((flag - 10) & 14) << 7) + byte(glyphs)),
            0,
        ),
        20..84 => {
            let (high, low) = (i32::from(flag - 20), byte(glyphs));
            (
                signed(flag, 1 + (high & 0x30) + (low >> 4)),
                signed(flag >> 1, 1 + ((high & 0x0c) << 2) + (low & 0x0f)),
            )
        }
        84..120 => {
            let high = i32::from(flag - 84);
            let (x, y) = (byte(glyphs), byte(glyphs));
            (
                signed(flag, 1 + ((high / 12) << 8) + x),
                signed(flag >> 1, 1 + (((high % 12) >> 2) << 8) + y),
            )
        }
        120..124 => {
            let (first, second, third) = (byte(glyphs), byte(glyphs), byte(glyphs));
            (
                signed(flag, (first << 4) + (second >> 4)),
                signed(flag >> 1, ((second & 0x0f) << 8) + third),
            )
        }
        _ => {
            let x = (byte(glyphs) << 8) + byte(glyphs);
            let y = (byte(glyphs) << 8) + byte(glyphs);
            (signed(flag, x), signed(flag >> 1, y))
        }
    }
}

/// The bounding box of `points` as x min, y min, x max, y max; zeros when empty.
fn bounds(points: &[Point]) -> [i16; 4] {
    let Some(first) = points.first() else {
        return [0; 4];
    };
    let start = [first.x, first.y, first.x, first.y];
    let [x_min, y_min, x_max, y_max] = points.iter().fold(start, |[a, b, c, d], point| {
        [
            a.min(point.x),
            b.min(point.y),
            c.max(point.x),
            d.max(point.y),
        ]
    });
    [x_min, y_min, x_max, y_max].map(|value| value as i16)
}

/// Writes a coordinate delta in TrueType form: a byte with the sign in the flag when under 256,
/// nothing when zero, else a word.
fn write_move(delta: i32, short: u8, same_or_positive: u8, flag: &mut u8, out: &mut Vec<u8>) {
    if delta == 0 {
        *flag |= same_or_positive;
    } else if delta.abs() < 256 {
        *flag |= short | if delta > 0 { same_or_positive } else { 0 };
        out.push(delta.unsigned_abs() as u8);
    } else {
        out.extend((delta as i16).to_be_bytes());
    }
}

/// Writes `points` as a simple glyph's flags, then x deltas, then y deltas.
fn write_points(points: &[Point], overlap: bool, out: &mut Vec<u8>) {
    let (mut flags, mut xs, mut ys) = (Vec::new(), Vec::new(), Vec::new());
    let mut last = (0, 0);
    for (index, point) in points.iter().enumerate() {
        let mut flag = if point.on_curve { ON_CURVE } else { 0 };
        if index == 0 && overlap {
            flag |= OVERLAP_SIMPLE;
        }
        write_move(
            point.x - last.0,
            X_SHORT,
            X_SAME_OR_POSITIVE,
            &mut flag,
            &mut xs,
        );
        write_move(
            point.y - last.1,
            Y_SHORT,
            Y_SAME_OR_POSITIVE,
            &mut flag,
            &mut ys,
        );
        flags.push(flag);
        last = (point.x, point.y);
    }
    out.extend(flags);
    out.extend(xs);
    out.extend(ys);
}

/// The transformed table's streams, consumed as glyphs are rebuilt.
struct Streams<'a> {
    contours: Stream<'a>,
    points: Stream<'a>,
    flags: Stream<'a>,
    glyphs: Stream<'a>,
    composites: Stream<'a>,
    /// Marks glyphs with an explicit box in `boxes` rather than one computed from points.
    box_bitmap: &'a [u8],
    boxes: Stream<'a>,
    instructions: Stream<'a>,
    overlap_bitmap: Option<&'a [u8]>,
}

impl<'a> Streams<'a> {
    /// Splits `table`, holding `count` glyphs, into its streams.
    fn of(table: &'a [u8], count: usize) -> Self {
        let length = |index: usize| u32_at(table, 8 + 4 * index) as usize;
        let mut at = HEADER_LENGTH;
        let mut next = |index: usize| {
            let stream = &table[at..at + length(index)];
            at += stream.len();
            stream
        };
        let (contours, points, flags, glyphs, composites) =
            (next(0), next(1), next(2), next(3), next(4));
        let boxes = next(5);
        let instructions = next(6);
        let box_bitmap_length = 4 * count.div_ceil(32);
        let overlap_bitmap = (u16_at(table, 2) & HAS_OVERLAP_BITMAP != 0)
            .then(|| &table[at..at + count.div_ceil(8)]);
        Self {
            contours: Stream::new(contours),
            points: Stream::new(points),
            flags: Stream::new(flags),
            glyphs: Stream::new(glyphs),
            composites: Stream::new(composites),
            box_bitmap: &boxes[..box_bitmap_length],
            boxes: Stream::new(&boxes[box_bitmap_length..]),
            instructions: Stream::new(instructions),
            overlap_bitmap,
        }
    }

    /// Glyph `index` in `glyf` form; empty for an empty glyph.
    fn glyph(&mut self, index: usize) -> Vec<u8> {
        match self.contours.i16() {
            0 => Vec::new(),
            -1 => self.composite(index),
            contours => self.simple(index, contours as usize),
        }
    }

    /// Glyph `index`'s explicit box, if it has one.
    fn given_box(&mut self, index: usize) -> Option<[i16; 4]> {
        bit(self.box_bitmap, index).then(|| {
            let boxes = &mut self.boxes;
            [boxes.i16(), boxes.i16(), boxes.i16(), boxes.i16()]
        })
    }

    /// Copies a glyph's instructions, whose length is in the glyph stream, as `glyf` stores
    /// them: length, then bytes.
    fn write_instructions(&mut self, out: &mut Vec<u8>) {
        let length = self.glyphs.u255();
        out.extend((length as u16).to_be_bytes());
        out.extend(self.instructions.take(length));
    }

    /// Rebuilds a simple glyph with `contours` contours.
    fn simple(&mut self, index: usize, contours: usize) -> Vec<u8> {
        let mut ends = Vec::with_capacity(contours);
        let mut total = 0;
        for _ in 0..contours {
            total += self.points.u255();
            ends.push(total - 1);
        }
        let mut last = (0, 0);
        let points: Vec<Point> = (0..total)
            .map(|_| {
                let flag = self.flags.u8();
                let (dx, dy) = triplet(flag, &mut self.glyphs);
                last = (last.0 + dx, last.1 + dy);
                Point {
                    x: last.0,
                    y: last.1,
                    on_curve: flag & 0x80 == 0,
                }
            })
            .collect();
        let found = bounds(&points);
        let shown = self.given_box(index).unwrap_or(found);
        let mut out = Vec::new();
        out.extend((contours as i16).to_be_bytes());
        shown
            .iter()
            .for_each(|value| out.extend(value.to_be_bytes()));
        ends.iter()
            .for_each(|end| out.extend((*end as u16).to_be_bytes()));
        self.write_instructions(&mut out);
        let overlap = self.overlap_bitmap.is_some_and(|bitmap| bit(bitmap, index));
        write_points(&points, overlap, &mut out);
        out
    }

    /// One composite component in `glyf` form, with its flags.
    fn component(&mut self) -> (u16, &'a [u8]) {
        let flags = u16_at(self.composites.data, self.composites.at);
        let arguments = if flags & ARGS_ARE_WORDS != 0 { 4 } else { 2 };
        let scale = match () {
            () if flags & HAS_SCALE != 0 => 2,
            () if flags & HAS_X_AND_Y_SCALE != 0 => 4,
            () if flags & HAS_TWO_BY_TWO != 0 => 8,
            () => 0,
        };
        (flags, self.composites.take(4 + arguments + scale))
    }

    /// Rebuilds a composite glyph. Its box is always explicit, and instructions follow the
    /// components when a component flag says so.
    fn composite(&mut self, index: usize) -> Vec<u8> {
        let shown = self
            .given_box(index)
            .expect("a composite glyph's box is given");
        let mut out = Vec::new();
        out.extend((-1i16).to_be_bytes());
        shown
            .iter()
            .for_each(|value| out.extend(value.to_be_bytes()));
        let mut instructed = false;
        loop {
            let (flags, component) = self.component();
            out.extend(component);
            instructed |= flags & HAS_INSTRUCTIONS != 0;
            if flags & MORE_COMPONENTS == 0 {
                break;
            }
        }
        if instructed {
            self.write_instructions(&mut out);
        }
        out
    }
}

/// Rebuilds `glyf` and `loca` from the transformed `table`, with `loca` in the format the table
/// names (the same as `head`'s).
pub fn rebuilt(table: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let count = usize::from(u16_at(table, 4));
    let long = u16_at(table, 6) != 0;
    let mut streams = Streams::of(table, count);
    let mut glyf = Vec::new();
    let mut starts = Vec::with_capacity(count + 1);
    for index in 0..count {
        starts.push(glyf.len());
        glyf.extend(streams.glyph(index));
        // Align each glyph to 4 bytes, which either `loca` format can address.
        glyf.resize(glyf.len().next_multiple_of(4), 0);
    }
    starts.push(glyf.len());
    let loca = starts
        .iter()
        .flat_map(|&start| {
            if long {
                (start as u32).to_be_bytes().to_vec()
            } else {
                ((start / 2) as u16).to_be_bytes().to_vec()
            }
        })
        .collect();
    (glyf, loca)
}
