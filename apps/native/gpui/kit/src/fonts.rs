//! The app's faces at the weights the screens use. Inter ships as one variable font
//! (`Inter-latin.woff2`, weight axis 100 to 900). GPUI picks a face by weight within a family
//! and can't set variable axes, so every weight would draw as the default instance (400). This
//! module builds static instances of the weights the app uses: outlines and advances are moved by
//! the font's variations (`gvar` through `avar`), the variation tables are dropped, and the weight
//! class and names are set. Kerning pairs are varied too (`GPOS` through `GDEF`'s variation
//! store), matching how Core Text kerns the variable font at that weight.
//!
//! No italic ships. Core Text slants the upright face with a matrix, but GPUI can't, so each
//! weight and the monospace also get an italic face with sheared outlines (each point's x moved
//! by `northroom_core::text::typeface::SLANT` of its height). Glyph positions come from the core
//! either way (`text`).
//!
//! The tables come from the core's WOFF2 decoder (`northroom_core::text::woff2`), so every platform
//! sees the same data; instancing is plain Rust over those tables.

use northroom_core::text::typeface::SLANT;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// The non-default weights the screens use. 650 is a plain-text file's title when read as a page
/// (`page::layout::lay_out_document`).
pub const WEIGHTS: [f32; 4] = [500.0, 600.0, 650.0, 700.0];

/// Every table of the WOFF2 face `file`, by tag, as the core decodes it.
fn font_tables(file: &[u8]) -> BTreeMap<u32, Vec<u8>> {
    northroom_core::text::woff2::tables(file)
        .into_iter()
        .map(|(tag, data)| (u32::from_be_bytes(tag), data))
        .collect()
}

/// The faces GPUI is given, built from the shipped WOFF2 files `inter` and `mono`: Inter's
/// default instance (400) and Geist Mono, static instances of the other weights, and a slanted
/// italic of every weight and the monospace (`SLANT`). Core Text reads WOFF2, so macOS gets the
/// shipped files as they are. DirectWrite and the Linux text system read only OpenType, so there
/// each file is rewritten, and Inter's default instance is made static too so its named
/// instances don't show up as extra faces.
pub fn faces(inter: &'static [u8], mono: &'static [u8]) -> Vec<Cow<'static, [u8]>> {
    let tables = font_tables(inter);
    let mono_tables = font_tables(mono);
    let shipped = if cfg!(target_os = "macos") {
        vec![Cow::Borrowed(inter), Cow::Borrowed(mono)]
    } else {
        vec![
            Cow::Owned(instance(&tables, 400.0, "Inter", "Regular", 0.0)),
            Cow::Owned(write(&mono_tables)),
        ]
    };
    let slant = SLANT as f32;
    shipped
        .into_iter()
        .chain(WEIGHTS.iter().map(|&weight| {
            Cow::Owned(instance(
                &tables,
                weight,
                "Inter",
                &format!("W{weight}"),
                0.0,
            ))
        }))
        .chain(std::iter::once(400.0).chain(WEIGHTS).map(|weight| {
            Cow::Owned(instance(
                &tables,
                weight,
                "Inter",
                &format!("W{weight} Italic"),
                slant,
            ))
        }))
        .chain(std::iter::once(Cow::Owned(instance(
            &mono_tables,
            400.0,
            "Geist Mono",
            "Italic",
            slant,
        ))))
        .collect()
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([data[at], data[at + 1]])
}

fn i16_at(data: &[u8], at: usize) -> i16 {
    i16::from_be_bytes([data[at], data[at + 1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn f2dot14(data: &[u8], at: usize) -> f32 {
    f32::from(i16_at(data, at)) / 16384.0
}

fn tag(name: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*name)
}

/// `weight` normalized to the weight axis (-1 to 1) through the `avar` map.
fn normalized(tables: &BTreeMap<u32, Vec<u8>>, weight: f32) -> f32 {
    let fvar = &tables[&tag(b"fvar")];
    let axes_at = usize::from(u16_at(fvar, 4));
    let (minimum, default, maximum) = (
        u32_at(fvar, axes_at + 4) as i32 as f32 / 65536.0,
        u32_at(fvar, axes_at + 8) as i32 as f32 / 65536.0,
        u32_at(fvar, axes_at + 12) as i32 as f32 / 65536.0,
    );
    let weight = weight.clamp(minimum, maximum);
    let value = if weight < default {
        (weight - default) / (default - minimum)
    } else if weight > default {
        (weight - default) / (maximum - default)
    } else {
        0.0
    };
    match tables.get(&tag(b"avar")) {
        Some(avar) => through_avar(avar, value),
        None => value,
    }
}

/// Maps `value` through the first axis's `avar` segment map (piecewise linear from/to pairs).
fn through_avar(avar: &[u8], value: f32) -> f32 {
    let count = usize::from(u16_at(avar, 8));
    let pairs: Vec<(f32, f32)> = (0..count)
        .map(|index| (f2dot14(avar, 10 + index * 4), f2dot14(avar, 12 + index * 4)))
        .collect();
    let Some(window) = pairs
        .windows(2)
        .find(|pair| value >= pair[0].0 && value <= pair[1].0)
    else {
        return value;
    };
    let ((from0, to0), (from1, to1)) = (window[0], window[1]);
    if from1 == from0 {
        to0
    } else {
        to0 + (value - from0) / (from1 - from0) * (to1 - to0)
    }
}

/// How much a tuple variation applies at `at` on the single axis.
fn scalar(at: f32, peak: f32, region: Option<(f32, f32)>) -> f32 {
    if peak == 0.0 {
        return 1.0;
    }
    if at == 0.0 {
        return 0.0;
    }
    match region {
        Some((start, end)) if at < start || at > end => 0.0,
        Some(_) if at == peak => 1.0,
        Some((start, _)) if at < peak => (at - start) / (peak - start),
        Some((_, end)) => (end - at) / (end - peak),
        None if at.signum() != peak.signum() || at.abs() > peak.abs() => 0.0,
        None => at / peak,
    }
}

/// Packed point numbers from `at`; `None` means every point.
fn points(data: &[u8], at: &mut usize) -> Option<Vec<usize>> {
    let first = data[*at];
    *at += 1;
    if first == 0 {
        return None;
    }
    let count = if first & 0x80 != 0 {
        let count = (usize::from(first & 0x7F) << 8) | usize::from(data[*at]);
        *at += 1;
        count
    } else {
        usize::from(first)
    };
    let mut numbers = Vec::with_capacity(count);
    let mut last = 0usize;
    while numbers.len() < count {
        let control = data[*at];
        *at += 1;
        let run = usize::from(control & 0x7F) + 1;
        for _ in 0..run {
            let step = if control & 0x80 != 0 {
                let step = usize::from(u16_at(data, *at));
                *at += 2;
                step
            } else {
                let step = usize::from(data[*at]);
                *at += 1;
                step
            };
            last += step;
            numbers.push(last);
        }
    }
    Some(numbers)
}

fn deltas(data: &[u8], at: &mut usize, count: usize) -> Vec<f32> {
    let mut values = Vec::with_capacity(count);
    while values.len() < count {
        let control = data[*at];
        *at += 1;
        let run = usize::from(control & 0x3F) + 1;
        for _ in 0..run {
            if control & 0x80 != 0 {
                values.push(0.0);
            } else if control & 0x40 != 0 {
                values.push(f32::from(i16_at(data, *at)));
                *at += 2;
            } else {
                values.push(f32::from(data[*at] as i8));
                *at += 1;
            }
        }
    }
    values
}

/// Interpolates deltas for untouched points from their neighbours on the same contour (IUP).
fn infer(original: &[(f32, f32)], ends: &[usize], touched: &[bool], moved: &mut [(f32, f32)]) {
    let mut first = 0;
    for &last in ends {
        let marks: Vec<usize> = (first..=last).filter(|&point| touched[point]).collect();
        // A contour with no touched points doesn't move.
        if !marks.is_empty() && marks.len() <= last - first {
            break_contour(original, touched, moved, &marks, first, last);
        }
        first = last + 1;
    }
}

/// Infers the untouched points of one contour `first..=last`, between the touched `marks`.
fn break_contour(
    original: &[(f32, f32)],
    touched: &[bool],
    moved: &mut [(f32, f32)],
    marks: &[usize],
    first: usize,
    last: usize,
) {
    let length = last + 1 - first;
    for (index, &mark) in marks.iter().enumerate() {
        let next = marks[(index + 1) % marks.len()];
        // Points strictly between `mark` and `next`, wrapping round the contour.
        let mut point = round_after(mark, first, last);
        let mut steps = 0;
        while point != next && steps < length {
            if !touched[point] {
                let (from, to, here) = (original[mark], original[next], original[point]);
                let (moved_from, moved_to) = (moved[mark], moved[next]);
                moved[point] = (
                    inferred_delta((from.0, to.0), (moved_from.0, moved_to.0), here.0),
                    inferred_delta((from.1, to.1), (moved_from.1, moved_to.1), here.1),
                );
            }
            point = round_after(point, first, last);
            steps += 1;
        }
    }
}

/// The point after `point` on the contour `first..=last`, wrapping from last to first.
#[inline]
fn round_after(point: usize, first: usize, last: usize) -> usize {
    if point == last { first } else { point + 1 }
}

/// An untouched point's delta on one axis from the two touched points around it (`ends`, moved
/// by `deltas`): interpolated between them, or the nearer one's delta outside them.
#[inline]
fn inferred_delta(ends: (f32, f32), deltas: (f32, f32), here: f32) -> f32 {
    let ((a, b), (da, db)) = (ends, deltas);
    if a == b {
        return if da == db { da } else { 0.0 };
    }
    let (low, high, dlow, dhigh) = if a < b {
        (a, b, da, db)
    } else {
        (b, a, db, da)
    };
    if here <= low {
        dlow
    } else if here >= high {
        dhigh
    } else {
        dlow + (here - low) / (high - low) * (dhigh - dlow)
    }
}

/// Deltas at `at` for glyph `glyph`'s `count` points (outline or component points, then the four
/// phantom points).
fn glyph_deltas(
    gvar: &[u8],
    glyph: usize,
    count: usize,
    at: f32,
    original: &[(f32, f32)],
    ends: &[usize],
) -> Vec<(f32, f32)> {
    let mut total = vec![(0.0f32, 0.0f32); count];
    let Some(base) = glyph_variations(gvar, glyph) else {
        return total;
    };
    let shared_at = u32_at(gvar, 8) as usize;
    let header = u16_at(gvar, base);
    let tuples = usize::from(header & 0x0FFF);
    let mut serialized = base + usize::from(u16_at(gvar, base + 2));
    let shared_points = if header & 0x8000 != 0 {
        points(gvar, &mut serialized)
    } else {
        None
    };
    let mut cursor = base + 4;
    for _ in 0..tuples {
        let tuple = TupleHeader::read(gvar, shared_at, &mut cursor);
        let weight = scalar(at, tuple.peak, tuple.region);
        let mut read = serialized;
        serialized += tuple.size;
        if weight == 0.0 {
            continue;
        }
        let numbers = tuple.point_numbers(gvar, &mut read, shared_points.as_deref());
        let moved = tuple_moves(gvar, &mut read, numbers, original, ends, count);
        for (sum, (dx, dy)) in total.iter_mut().zip(moved) {
            sum.0 += dx * weight;
            sum.1 += dy * weight;
        }
    }
    total
}

/// The offset of glyph `glyph`'s variation data in `gvar`; `None` if it has none.
fn glyph_variations(gvar: &[u8], glyph: usize) -> Option<usize> {
    let glyph_count = usize::from(u16_at(gvar, 12));
    let long = u16_at(gvar, 14) & 1 != 0;
    let data_at = u32_at(gvar, 16) as usize;
    if glyph >= glyph_count {
        return None;
    }
    let offset = |index: usize| {
        if long {
            u32_at(gvar, 20 + index * 4) as usize
        } else {
            usize::from(u16_at(gvar, 20 + index * 2)) * 2
        }
    };
    let (begin, end) = (offset(glyph), offset(glyph + 1));
    (begin != end).then_some(data_at + begin)
}

/// One tuple variation's header in a glyph's variation data.
struct TupleHeader {
    /// Size of its serialized data in bytes.
    size: usize,
    /// Peak on the weight axis.
    peak: f32,
    /// Intermediate region on the weight axis, if any.
    region: Option<(f32, f32)>,
    /// Whether its data starts with private point numbers instead of the shared ones.
    own_points: bool,
}

impl TupleHeader {
    /// Reads the header at `cursor` and advances past it. `shared_at` is the offset of `gvar`'s
    /// shared tuples.
    fn read(gvar: &[u8], shared_at: usize, cursor: &mut usize) -> Self {
        let size = usize::from(u16_at(gvar, *cursor));
        let index = u16_at(gvar, *cursor + 2);
        *cursor += 4;
        let peak = if index & 0x8000 != 0 {
            let peak = f2dot14(gvar, *cursor);
            *cursor += 2;
            peak
        } else {
            f2dot14(gvar, shared_at + usize::from(index & 0x0FFF) * 2)
        };
        let region = if index & 0x4000 != 0 {
            let region = (f2dot14(gvar, *cursor), f2dot14(gvar, *cursor + 2));
            *cursor += 4;
            Some(region)
        } else {
            None
        };
        Self {
            size,
            peak,
            region,
            own_points: index & 0x2000 != 0,
        }
    }

    /// The points this tuple moves: its private numbers read from `read` (advancing it), or the
    /// glyph's `shared` ones. `None` means every point.
    fn point_numbers(
        &self,
        gvar: &[u8],
        read: &mut usize,
        shared: Option<&[usize]>,
    ) -> Option<Vec<usize>> {
        if self.own_points {
            points(gvar, read)
        } else {
            shared.map(<[usize]>::to_vec)
        }
    }
}

/// The per-point deltas of one tuple variation over `count` points: the deltas at `read` for
/// `numbers` (all points if `None`), with untouched outline points interpolated.
fn tuple_moves(
    gvar: &[u8],
    read: &mut usize,
    numbers: Option<Vec<usize>>,
    original: &[(f32, f32)],
    ends: &[usize],
    count: usize,
) -> Vec<(f32, f32)> {
    let numbers: Vec<usize> = numbers.unwrap_or_else(|| (0..count).collect());
    let xs = deltas(gvar, read, numbers.len());
    let ys = deltas(gvar, read, numbers.len());
    let mut moved = vec![(0.0f32, 0.0f32); count];
    let mut touched = vec![false; count];
    for (position, &point) in numbers.iter().enumerate() {
        if point < count {
            moved[point] = (xs[position], ys[position]);
            touched[point] = true;
        }
    }
    // Untouched outline points move with their neighbours.
    if numbers.len() < count && !ends.is_empty() {
        let outline = ends.last().map_or(0, |&last| last + 1);
        infer(
            &original[..outline],
            ends,
            &touched[..outline],
            &mut moved[..outline],
        );
    }
    moved
}

struct Glyph {
    bytes: Vec<u8>,
    advance: u16,
    lsb: i16,
}

/// Glyph `glyph` of `glyf` instanced at `at` and sheared by `slant` (x moved by that fraction of
/// y), with its advance and left side bearing.
fn instance_glyph(
    tables: &BTreeMap<u32, Vec<u8>>,
    glyph: usize,
    at: f32,
    metric: (u16, i16),
    slant: f32,
) -> Glyph {
    let (glyf, loca, head) = (
        &tables[&tag(b"glyf")],
        &tables[&tag(b"loca")],
        &tables[&tag(b"head")],
    );
    let gvar = &tables[&tag(b"gvar")];
    let long = i16_at(head, 50) == 1;
    let offset = |index: usize| {
        if long {
            u32_at(loca, index * 4) as usize
        } else {
            usize::from(u16_at(loca, index * 2)) * 2
        }
    };
    let (begin, end) = (offset(glyph), offset(glyph + 1));
    let (advance, lsb) = metric;
    if begin == end {
        // No outline, so only the advance can vary.
        let phantoms = glyph_deltas(gvar, glyph, 4, at, &[], &[]);
        let width = (f32::from(advance) + phantoms[1].0 - phantoms[0].0)
            .round()
            .max(0.0) as u16;
        return Glyph {
            bytes: Vec::new(),
            advance: width,
            lsb,
        };
    }
    let data = &glyf[begin..end];
    if i16_at(data, 0) >= 0 {
        instance_outline(data, (gvar, glyph, at), metric, slant)
    } else {
        instance_composite(data, (gvar, glyph, at), metric, slant)
    }
}

/// A simple glyph (`data`, its `glyf` entry) instanced from `gvar` at `at` and sheared.
fn instance_outline(
    data: &[u8],
    (gvar, glyph, at): (&[u8], usize, f32),
    (advance, lsb): (u16, i16),
    slant: f32,
) -> Glyph {
    let contours = i16_at(data, 0) as usize;
    let x_min = i16_at(data, 2);
    let ends: Vec<usize> = (0..contours)
        .map(|index| usize::from(u16_at(data, 10 + index * 2)))
        .collect();
    let count = ends.last().map_or(0, |&last| last + 1);
    let mut at_byte = 10 + contours * 2;
    let instructions = usize::from(u16_at(data, at_byte));
    at_byte += 2 + instructions;
    let flags = read_flags(data, &mut at_byte, count);
    let xs = read_coordinates(data, &flags, 0x02, 0x10, &mut at_byte);
    let ys = read_coordinates(data, &flags, 0x04, 0x20, &mut at_byte);
    let original: Vec<(f32, f32)> = xs.iter().copied().zip(ys.iter().copied()).collect();
    let phantom_left = f32::from(x_min) - f32::from(lsb);
    let mut with_phantoms = original.clone();
    with_phantoms.extend([
        (phantom_left, 0.0),
        (phantom_left + f32::from(advance), 0.0),
        (0.0, 0.0),
        (0.0, 0.0),
    ]);
    let moves = glyph_deltas(gvar, glyph, count + 4, at, &with_phantoms, &ends);
    let points: Vec<(i32, i32)> = original
        .iter()
        .zip(&moves)
        .map(|(&(x, y), &(dx, dy))| {
            (
                (x + dx + slant * (y + dy)).round() as i32,
                (y + dy).round() as i32,
            )
        })
        .collect();
    let left = (phantom_left + moves[count].0).round();
    let right = (phantom_left + f32::from(advance) + moves[count + 1].0).round();
    let bounds = point_bounds(&points);
    Glyph {
        bytes: write_outline(&ends, &flags, &points, bounds),
        advance: (right - left).max(0.0) as u16,
        lsb: (bounds[0] as f32 - left) as i16,
    }
}

/// Reads `count` point flags at `at_byte`, expanding repeats, and advances past them.
fn read_flags(data: &[u8], at_byte: &mut usize, count: usize) -> Vec<u8> {
    let mut flags = Vec::with_capacity(count);
    while flags.len() < count {
        let flag = data[*at_byte];
        *at_byte += 1;
        flags.push(flag);
        if flag & 0x08 != 0 {
            let repeat = data[*at_byte];
            *at_byte += 1;
            for _ in 0..repeat {
                flags.push(flag);
            }
        }
    }
    flags
}

/// Reads one axis's coordinates at `at_byte` and advances past them. A point's step is a byte
/// when its flag has `short` (with `same` meaning positive), otherwise 16 bits, or zero when
/// `same` alone is set.
fn read_coordinates(
    data: &[u8],
    flags: &[u8],
    short: u8,
    same: u8,
    at_byte: &mut usize,
) -> Vec<f32> {
    let mut value = 0i32;
    flags
        .iter()
        .map(|&flag| {
            if flag & short != 0 {
                let step = i32::from(data[*at_byte]);
                *at_byte += 1;
                value += if flag & same != 0 { step } else { -step };
            } else if flag & same == 0 {
                value += i32::from(i16_at(data, *at_byte));
                *at_byte += 2;
            }
            value as f32
        })
        .collect()
}

/// The bounding box of `points`: min x, min y, max x, max y.
fn point_bounds(points: &[(i32, i32)]) -> [i32; 4] {
    let (min_x, max_x) = points
        .iter()
        .fold((i32::MAX, i32::MIN), |(low, high), point| {
            (low.min(point.0), high.max(point.0))
        });
    let (min_y, max_y) = points
        .iter()
        .fold((i32::MAX, i32::MIN), |(low, high), point| {
            (low.min(point.1), high.max(point.1))
        });
    [min_x, min_y, max_x, max_y]
}

/// Writes a simple glyph's `glyf` entry uncompressed: one flag per point and 16-bit coordinate
/// steps.
fn write_outline(ends: &[usize], flags: &[u8], points: &[(i32, i32)], bounds: [i32; 4]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend((ends.len() as i16).to_be_bytes());
    for value in bounds {
        bytes.extend((value as i16).to_be_bytes());
    }
    for &end in ends {
        bytes.extend((end as u16).to_be_bytes());
    }
    bytes.extend(0u16.to_be_bytes());
    bytes.extend(flags.iter().map(|flag| flag & 0x41));
    let mut last = 0;
    for point in points {
        bytes.extend(((point.0 - last) as i16).to_be_bytes());
        last = point.0;
    }
    last = 0;
    for point in points {
        bytes.extend(((point.1 - last) as i16).to_be_bytes());
        last = point.1;
    }
    pad_to_words(&mut bytes);
    bytes
}

fn pad_to_words(bytes: &mut Vec<u8>) {
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}

/// One component of a composite glyph, with its scale bytes kept verbatim.
struct Component {
    flags: u16,
    index: u16,
    dx: i16,
    dy: i16,
    scale: Vec<u8>,
}

/// A composite glyph (`data`, its `glyf` entry). Each component offset is one point, varied from
/// `gvar` at `at` and sheared with the outlines it places.
fn instance_composite(
    data: &[u8],
    (gvar, glyph, at): (&[u8], usize, f32),
    (advance, lsb): (u16, i16),
    slant: f32,
) -> Glyph {
    let x_min = i16_at(data, 2);
    let components = read_components(data);
    let count = components.len();
    let phantom_left = f32::from(x_min) - f32::from(lsb);
    let moves = glyph_deltas(gvar, glyph, count + 4, at, &[], &[]);
    let mut bytes = Vec::new();
    bytes.extend(data[0..10].iter());
    for (component, &moved) in components.iter().zip(&moves) {
        write_component(&mut bytes, component, moved, slant);
    }
    pad_to_words(&mut bytes);
    let left = (phantom_left + moves[count].0).round();
    let right = (phantom_left + f32::from(advance) + moves[count + 1].0).round();
    Glyph {
        bytes,
        advance: (right - left).max(0.0) as u16,
        lsb: (f32::from(lsb) + phantom_left - left) as i16,
    }
}

/// The components of the composite glyph `data`.
fn read_components(data: &[u8]) -> Vec<Component> {
    let mut components = Vec::new();
    let mut at_byte = 10;
    loop {
        let flags = u16_at(data, at_byte);
        let index = u16_at(data, at_byte + 2);
        at_byte += 4;
        let (dx, dy) = if flags & 0x0001 != 0 {
            let pair = (i16_at(data, at_byte), i16_at(data, at_byte + 2));
            at_byte += 4;
            pair
        } else {
            let pair = (
                i16::from(data[at_byte] as i8),
                i16::from(data[at_byte + 1] as i8),
            );
            at_byte += 2;
            pair
        };
        let scale_bytes = scale_size(flags);
        let scale = data[at_byte..at_byte + scale_bytes].to_vec();
        at_byte += scale_bytes;
        components.push(Component {
            flags,
            index,
            dx,
            dy,
            scale,
        });
        if flags & 0x0020 == 0 {
            return components;
        }
    }
}

/// Size of a component's scale by its flags: one scale, separate x and y scales, or a 2 × 2
/// matrix, each F2Dot14.
fn scale_size(flags: u16) -> usize {
    if flags & 0x0008 != 0 {
        2
    } else if flags & 0x0040 != 0 {
        4
    } else if flags & 0x0080 != 0 {
        8
    } else {
        0
    }
}

/// Writes `component` with its offset moved by `moved` when it is an x/y offset. Offsets are
/// written as words, with no instructions after the last component.
fn write_component(bytes: &mut Vec<u8>, component: &Component, moved: (f32, f32), slant: f32) {
    let moved = if component.flags & 0x0002 != 0 {
        moved
    } else {
        (0.0, 0.0)
    };
    let flags = (component.flags | 0x0001) & !0x0100;
    bytes.extend(flags.to_be_bytes());
    bytes.extend(component.index.to_be_bytes());
    let (x, y) = (
        f32::from(component.dx) + moved.0,
        f32::from(component.dy) + moved.1,
    );
    // The offset shears with the outlines it places.
    let x = if flags & 0x0002 != 0 {
        x + slant * y
    } else {
        x
    };
    bytes.extend((x.round() as i16).to_be_bytes());
    bytes.extend((y.round() as i16).to_be_bytes());
    bytes.extend(&component.scale);
}

/// A `name` table for `family` and `style`.
fn names(family: &str, style: &str) -> Vec<u8> {
    let full = format!("{family} {style}");
    let postscript = format!("{family}-{style}").replace(' ', "");
    let records: [(u16, &str); 4] = [(1, family), (2, style), (4, &full), (6, &postscript)];
    let mut strings = Vec::new();
    let mut table = Vec::new();
    table.extend(0u16.to_be_bytes());
    table.extend((records.len() as u16).to_be_bytes());
    table.extend((6 + records.len() as u16 * 12).to_be_bytes());
    for (id, words) in records {
        let encoded: Vec<u8> = words.encode_utf16().flat_map(u16::to_be_bytes).collect();
        for value in [
            3u16,
            1,
            0x409,
            id,
            encoded.len() as u16,
            strings.len() as u16,
        ] {
            table.extend(value.to_be_bytes());
        }
        strings.extend(encoded);
    }
    table.extend(strings);
    table
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

/// An OpenType file holding `tables`.
fn write(tables: &BTreeMap<u32, Vec<u8>>) -> Vec<u8> {
    let count = tables.len() as u16;
    let selector = 15 - count.leading_zeros() as u16;
    let range = (1u16 << selector) * 16;
    let mut file = Vec::new();
    for value in [
        0x0001u16,
        0x0000,
        count,
        range,
        selector,
        count * 16 - range,
    ] {
        file.extend(value.to_be_bytes());
    }
    let mut offset = 12 + usize::from(count) * 16;
    let mut body = Vec::new();
    for (name, data) in tables {
        file.extend(name.to_be_bytes());
        file.extend(checksum(data).to_be_bytes());
        file.extend((offset as u32).to_be_bytes());
        file.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + usize::from(count) * 16 + body.len();
    }
    file.extend(body);
    file
}

/// A static instance of a variable font at `weight`, sheared by `slant`. A nonzero `slant` marks
/// the face italic.
pub fn instance(
    tables: &BTreeMap<u32, Vec<u8>>,
    weight: f32,
    family: &str,
    style: &str,
    slant: f32,
) -> Vec<u8> {
    let at = normalized(tables, weight);
    let glyph_count = usize::from(u16_at(&tables[&tag(b"maxp")], 4));
    let hhea = &tables[&tag(b"hhea")];
    let hmtx = &tables[&tag(b"hmtx")];
    let metrics_count = usize::from(u16_at(hhea, 34));
    let metric = |glyph: usize| {
        let advance = u16_at(hmtx, glyph.min(metrics_count - 1) * 4);
        let lsb = if glyph < metrics_count {
            i16_at(hmtx, glyph * 4 + 2)
        } else {
            i16_at(hmtx, metrics_count * 4 + (glyph - metrics_count) * 2)
        };
        (advance, lsb)
    };
    let glyphs: Vec<Glyph> = (0..glyph_count)
        .map(|glyph| instance_glyph(tables, glyph, at, metric(glyph), slant))
        .collect();
    let mut out: BTreeMap<u32, Vec<u8>> = tables
        .iter()
        .filter(|(name, _)| {
            ![
                b"fvar", b"gvar", b"avar", b"HVAR", b"MVAR", b"STAT", b"cvar", b"DSIG",
            ]
            .iter()
            .any(|dropped| **name == tag(dropped))
        })
        .map(|(name, data)| (*name, data.clone()))
        .collect();
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    let mut hmtx = Vec::new();
    for glyph in &glyphs {
        loca.extend((glyf.len() as u32).to_be_bytes());
        glyf.extend(&glyph.bytes);
        hmtx.extend(glyph.advance.to_be_bytes());
        hmtx.extend(glyph.lsb.to_be_bytes());
    }
    loca.extend((glyf.len() as u32).to_be_bytes());
    let widest = glyphs.iter().map(|glyph| glyph.advance).max().unwrap_or(0);
    out.insert(tag(b"glyf"), glyf);
    out.insert(tag(b"loca"), loca);
    out.insert(tag(b"hmtx"), hmtx);
    let mut head = out[&tag(b"head")].clone();
    head[50..52].copy_from_slice(&1i16.to_be_bytes());
    head[8..12].copy_from_slice(&[0, 0, 0, 0]);
    out.insert(tag(b"head"), head);
    let mut hhea = out[&tag(b"hhea")].clone();
    hhea[10..12].copy_from_slice(&widest.to_be_bytes());
    hhea[34..36].copy_from_slice(&(glyph_count as u16).to_be_bytes());
    out.insert(tag(b"hhea"), hhea);
    if let Some(mut os2) = out.get(&tag(b"OS/2")).cloned() {
        os2[4..6].copy_from_slice(&(weight as u16).to_be_bytes());
        if slant != 0.0 {
            // fsSelection: italic, not regular.
            let selection = (u16_at(&os2, 62) | 0x0001) & !0x0040;
            os2[62..64].copy_from_slice(&selection.to_be_bytes());
        }
        out.insert(tag(b"OS/2"), os2);
    }
    if slant != 0.0 {
        // Mark `head` italic and set `post`'s italic angle in 16.16 degrees (negative leans
        // forward).
        let mut head = out[&tag(b"head")].clone();
        let style = u16_at(&head, 44) | 0x0002;
        head[44..46].copy_from_slice(&style.to_be_bytes());
        out.insert(tag(b"head"), head);
        let mut post = out[&tag(b"post")].clone();
        let angle = (-slant.atan().to_degrees() * 65536.0).round() as i32;
        post[4..8].copy_from_slice(&angle.to_be_bytes());
        out.insert(tag(b"post"), post);
    }
    out.insert(tag(b"name"), names(family, style));
    if let (Some(gpos), Some(gdef)) = (tables.get(&tag(b"GPOS")), tables.get(&tag(b"GDEF"))) {
        out.insert(tag(b"GPOS"), varied_kerning(gpos, gdef, at));
    }
    write(&out)
}

/// `GPOS` with each pair's x advance (`PairPos`, direct or via an extension) varied at `at`
/// (`VariationIndex` device tables into `GDEF`'s item variation store). Nothing else changes.
fn varied_kerning(gpos: &[u8], gdef: &[u8], at: f32) -> Vec<u8> {
    let mut out = gpos.to_vec();
    // GDEF 1.3 stores the item variation store offset after its five other offsets.
    if u16_at(gdef, 0) != 1 || u16_at(gdef, 2) < 3 {
        return out;
    }
    let store = u32_at(gdef, 14) as usize;
    if store == 0 {
        return out;
    }
    let delta = |outer: usize, inner: usize| item_delta(gdef, store, outer, inner, at);
    let lookups = usize::from(u16_at(gpos, 8));
    for lookup in 0..usize::from(u16_at(gpos, lookups)) {
        let table = lookups + usize::from(u16_at(gpos, lookups + 2 + lookup * 2));
        let kind = u16_at(gpos, table);
        for sub in 0..usize::from(u16_at(gpos, table + 4)) {
            let start = table + usize::from(u16_at(gpos, table + 6 + sub * 2));
            let (kind, start) = through_extension(gpos, kind, start);
            if kind == 2 {
                vary_pairs(gpos, &mut out, start, &delta);
            }
        }
    }
    out
}

/// The lookup type and start of the subtable at `start` in a lookup of type `kind`, following
/// extensions (type 9) to the real subtable.
fn through_extension(gpos: &[u8], kind: u16, start: usize) -> (u16, usize) {
    if kind == 9 {
        (
            u16_at(gpos, start + 2),
            start + u32_at(gpos, start + 4) as usize,
        )
    } else {
        (kind, start)
    }
}

/// Size of a value record of `format`, and the offsets of its x advance and that advance's
/// device table.
fn value_record(format: u16) -> (usize, Option<usize>, Option<usize>) {
    let field =
        |bit: u16| (format & bit != 0).then(|| ((format & (bit - 1)).count_ones() * 2) as usize);
    (
        (format.count_ones() * 2) as usize,
        field(0x0004),
        field(0x0040),
    )
}

/// Varies the first value of each pair in the `PairPos` subtable at `start` (both formats).
fn vary_pairs(gpos: &[u8], out: &mut [u8], start: usize, delta: &dyn Fn(usize, usize) -> f32) {
    let format = u16_at(gpos, start);
    let (first, second) = (u16_at(gpos, start + 4), u16_at(gpos, start + 6));
    let (first_size, advance, device) = value_record(first);
    let (second_size, ..) = value_record(second);
    let (Some(advance), Some(device)) = (advance, device) else {
        return;
    };
    let pair_advance = PairAdvance {
        start,
        advance,
        device,
    };
    match format {
        1 => {
            for set in 0..usize::from(u16_at(gpos, start + 8)) {
                let pairs = start + usize::from(u16_at(gpos, start + 10 + set * 2));
                let size = 2 + first_size + second_size;
                for pair in 0..usize::from(u16_at(gpos, pairs)) {
                    pair_advance.vary(gpos, out, pairs + 2 + pair * size + 2, delta);
                }
            }
        }
        2 => {
            let (classes, seconds) = (
                usize::from(u16_at(gpos, start + 12)),
                usize::from(u16_at(gpos, start + 14)),
            );
            let size = first_size + second_size;
            for record in 0..classes * seconds {
                pair_advance.vary(gpos, out, start + 16 + record * size, delta);
            }
        }
        _ => {}
    }
}

/// Offsets of the x advance and its device table in the first value record of each pair in the
/// `PairPos` subtable at `start`.
struct PairAdvance {
    start: usize,
    advance: usize,
    device: usize,
}

impl PairAdvance {
    /// Varies the x advance of the value record at `record` when its device table is a
    /// variation index.
    fn vary(
        &self,
        gpos: &[u8],
        out: &mut [u8],
        record: usize,
        delta: &dyn Fn(usize, usize) -> f32,
    ) {
        let (start, advance) = (self.start, self.advance);
        let table = usize::from(u16_at(gpos, record + self.device));
        // A device table of format 0x8000 is a variation index (outer, inner).
        if table == 0 || u16_at(gpos, start + table + 4) != 0x8000 {
            return;
        }
        let (outer, inner) = (
            usize::from(u16_at(gpos, start + table)),
            usize::from(u16_at(gpos, start + table + 2)),
        );
        let moved = f32::from(i16_at(gpos, record + advance)) + delta(outer, inner);
        out[record + advance..record + advance + 2]
            .copy_from_slice(&(moved.round() as i16).to_be_bytes());
    }
}

/// The delta of item (`outer`, `inner`) in the item variation store at `store` in `table`, at
/// the normalized weight `at`.
fn item_delta(table: &[u8], store: usize, outer: usize, inner: usize, at: f32) -> f32 {
    let regions = store + u32_at(table, store + 2) as usize;
    let axes = usize::from(u16_at(table, regions));
    if outer >= usize::from(u16_at(table, store + 6)) {
        return 0.0;
    }
    let data = store + u32_at(table, store + 8 + outer * 4) as usize;
    let (items, word_count, indexes) = (
        usize::from(u16_at(table, data)),
        u16_at(table, data + 2),
        usize::from(u16_at(table, data + 4)),
    );
    if inner >= items {
        return 0.0;
    }
    let row = DeltaRow::of(data, word_count, indexes, inner);
    (0..indexes)
        .map(|column| {
            // Weight is the first axis; ignore the region's other axes.
            let region = usize::from(u16_at(table, data + 6 + column * 2));
            let axis = regions + 4 + region * axes * 6;
            let (start, peak, end) = (
                f2dot14(table, axis),
                f2dot14(table, axis + 2),
                f2dot14(table, axis + 4),
            );
            row.delta(table, column) * scalar(at, peak, Some((start, end)))
        })
        .sum()
}

/// One delta row of item variation data. The first `words` columns are 32-bit when `long`,
/// otherwise 16-bit; the rest are half that.
struct DeltaRow {
    at: usize,
    words: usize,
    word: usize,
    small: usize,
    long: bool,
}

impl DeltaRow {
    /// Row `inner` of the item variation data at `data`, with `indexes` columns laid out per
    /// `word_count`.
    fn of(data: usize, word_count: u16, indexes: usize, inner: usize) -> Self {
        let long = word_count & 0x8000 != 0;
        let words = usize::from(word_count & 0x7fff);
        let (word, small) = if long { (4, 2) } else { (2, 1) };
        let row_size = words * word + (indexes - words) * small;
        Self {
            at: data + 6 + indexes * 2 + inner * row_size,
            words,
            word,
            small,
            long,
        }
    }

    fn delta(&self, table: &[u8], column: usize) -> f32 {
        if column < self.words {
            let at = self.at + column * self.word;
            return if self.long {
                u32_at(table, at) as i32 as f32
            } else {
                f32::from(i16_at(table, at))
            };
        }
        let at = self.at + self.words * self.word + (column - self.words) * self.small;
        if self.long {
            f32::from(i16_at(table, at))
        } else {
            f32::from(table[at] as i8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static INTER: &[u8] = include_bytes!("../../../core/assets/fonts/Inter-latin.woff2");
    static GEIST_MONO: &[u8] = include_bytes!("../../../core/assets/fonts/GeistMono-latin.woff2");

    /// The shipped faces decode into complete tables (`loca`'s last offset stays within
    /// `glyf`), and every weight builds from them.
    #[test]
    fn the_shipped_faces_unpack_whole_and_make_every_weight() {
        for file in [INTER, GEIST_MONO] {
            let tables = font_tables(file);
            let (glyf, loca) = (&tables[&tag(b"glyf")], &tables[&tag(b"loca")]);
            let last = if i16_at(&tables[&tag(b"head")], 50) == 1 {
                u32_at(loca, loca.len() - 4) as usize
            } else {
                usize::from(u16_at(loca, loca.len() - 2)) * 2
            };
            assert_eq!(last, glyf.len());
        }
        // The two shipped faces, the other weights, and an italic of each.
        assert_eq!(
            faces(INTER, GEIST_MONO).len(),
            2 + WEIGHTS.len() + 1 + WEIGHTS.len() + 1
        );
    }

    /// An italic face shears like Core Text's matrix: each point's x moves `SLANT` of its height,
    /// the advance stays, and the face is marked italic.
    #[test]
    fn a_slanted_face_leans_its_outlines_and_says_it_is_italic() {
        let tables = font_tables(INTER);
        let upright = read(&instance(&tables, 400.0, "Inter", "Regular", 0.0));
        let leaned = read(&instance(&tables, 400.0, "Inter", "Italic", SLANT as f32));
        // 'I' is one contour from baseline to cap, so its box's right edge moves by the shear
        // at its top.
        let glyph = 41;
        let (upright_box, leaned_box) = (glyph_box(&upright, glyph), glyph_box(&leaned, glyph));
        let top = f32::from(upright_box.3);
        assert!((f32::from(leaned_box.2 - upright_box.2) - SLANT as f32 * top).abs() <= 1.0);
        assert_eq!(advance(&upright, glyph), advance(&leaned, glyph));
        assert_eq!(u16_at(&leaned[&tag(b"OS/2")], 62) & 0x0041, 0x0001);
        assert_eq!(u16_at(&leaned[&tag(b"head")], 44) & 0x0002, 0x0002);
    }

    /// The tables of an OpenType file produced by `write`.
    fn read(file: &[u8]) -> BTreeMap<u32, Vec<u8>> {
        (0..usize::from(u16_at(file, 4)))
            .map(|index| {
                let entry = 12 + index * 16;
                let (offset, length) = (
                    u32_at(file, entry + 8) as usize,
                    u32_at(file, entry + 12) as usize,
                );
                (u32_at(file, entry), file[offset..offset + length].to_vec())
            })
            .collect()
    }

    /// Glyph `glyph`'s bounding box: min x, min y, max x, max y.
    fn glyph_box(tables: &BTreeMap<u32, Vec<u8>>, glyph: usize) -> (i16, i16, i16, i16) {
        let at = u32_at(&tables[&tag(b"loca")], glyph * 4) as usize;
        let glyf = &tables[&tag(b"glyf")];
        (
            i16_at(glyf, at + 2),
            i16_at(glyf, at + 4),
            i16_at(glyf, at + 6),
            i16_at(glyf, at + 8),
        )
    }

    fn advance(tables: &BTreeMap<u32, Vec<u8>>, glyph: usize) -> u16 {
        u16_at(&tables[&tag(b"hmtx")], glyph * 4)
    }
}
