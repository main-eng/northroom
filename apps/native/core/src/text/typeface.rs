//! The app's two bundled faces (Inter variable and Geist Mono, WOFF2), shaped and positioned the
//! way Core Text does it, so every shell on every platform measures text the same.
//!
//! Substitution matches Core Text via HarfRust: the font's `ccmp`, `locl` and `calt`, plus
//! `tnum` for tabular digits. Positions match Core Text exactly: an advance is the `hmtx`
//! advance plus `HVAR` at the weight, unrounded (weight normalized through `fvar` and `avar` in
//! floating point, then to 2.14 fixed point; region scalars in 16.16). Kerning is `GPOS` pairs
//! plus their variations, also unrounded. A fixed-pitch face (Geist Mono) advances by its
//! advance in ems truncated to 16.16. A slanted face (`Font::slanted`) is the upright one
//! sheared 0.2 by Core Text's matrix, which scales its kerning by 1.2.
//!
//! Characters neither face covers (◦, ▪, ⌘) fall back to the platform's faces, as in Core Text,
//! and the shell measures them (`set_fallback`).

use crate::board::card::Font;
use crate::Relock;
use harfrust::font::{BuiltinFontFuncs, FontFuncs};
use harfrust::{
    Feature, GlyphBuffer, ShapeOptions, ShapePlan, ShaperData, ShaperInstance, UnicodeBuffer,
};
use read_fonts::tables::variations::{DeltaSetIndex, ItemVariationStore};
use read_fonts::types::{F2Dot14, GlyphId, Tag};
use read_fonts::{FontRef, TableProvider};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Inter, a variable font with a weight axis from 100 to 900.
pub static INTER: &[u8] = include_bytes!("../../assets/fonts/Inter-latin.woff2");
/// Geist Mono, the app's monospace.
pub static GEIST_MONO: &[u8] = include_bytes!("../../assets/fonts/GeistMono-latin.woff2");

/// Shaping precision: positions come back in 1/65536 of a font unit, so nothing Core Text keeps
/// is rounded away.
const SUBUNITS: f64 = 65536.0;
/// Shear of a slanted face: x moves 0.2 of y (Core Text's matrix `c`).
pub const SLANT: f64 = 0.2;

/// A loaded face: its decoded file (`woff2::open_type`) and the data shaping needs.
struct Face {
    file: Vec<u8>,
    data: ShaperData,
    units_per_em: f64,
    fixed_pitch: bool,
    /// Ascent and descent in ems, rounded to 16.16 as Core Text reports them.
    ascent: f64,
    descent: f64,
}

impl Face {
    fn read(woff2: &[u8]) -> Self {
        let file = crate::text::woff2::open_type(&crate::text::woff2::tables(woff2));
        let font = FontRef::new(&file).expect("a shipped face reads");
        let units_per_em = f64::from(font.head().expect("a face's head").units_per_em());
        let fixed_pitch = font.post().expect("a face's post").is_fixed_pitch() != 0;
        let hhea = font.hhea().expect("a face's hhea");
        let ems = |units: i16| (f64::from(units) / units_per_em * 65536.0).round() / 65536.0;
        let (ascent, descent) = (
            ems(hhea.ascender().to_i16()),
            ems(-hhea.descender().to_i16()),
        );
        let data = ShaperData::new(&font);
        Self {
            file,
            data,
            units_per_em,
            fixed_pitch,
            ascent,
            descent,
        }
    }

    fn font(&self) -> FontRef<'_> {
        FontRef::new(&self.file).expect("a shipped face reads")
    }

    /// `weight` normalized on the face's axes as Core Text does it: through `fvar`'s range and
    /// `avar`'s map in floating point, then to 2.14 fixed point. Empty for a face with no
    /// axes.
    fn coords(&self, weight: f64) -> Vec<F2Dot14> {
        let font = self.font();
        let Ok(fvar) = font.fvar() else {
            return Vec::new();
        };
        let avar = font.avar().ok();
        let wght = Tag::new(b"wght");
        fvar.axes()
            .expect("the face's axes")
            .iter()
            .enumerate()
            .map(|(index, axis)| {
                let (minimum, default, maximum) = (
                    axis.min_value().to_f64(),
                    axis.default_value().to_f64(),
                    axis.max_value().to_f64(),
                );
                let user = if axis.axis_tag() == wght {
                    weight.clamp(minimum, maximum)
                } else {
                    default
                };
                let normalized = if user < default {
                    (user - default) / (default - minimum)
                } else if user > default {
                    (user - default) / (maximum - default)
                } else {
                    0.0
                };
                let mapped = match &avar {
                    Some(avar) => mapped(
                        avar.axis_segment_maps()
                            .get(index)
                            .expect("avar maps every axis")
                            .expect("an axis's map reads")
                            .axis_value_maps(),
                        normalized,
                    ),
                    None => normalized,
                };
                F2Dot14::from_bits((mapped * 16384.0).round() as i16)
            })
            .collect()
    }
}

/// Maps `value` through an axis's `avar` segments, piecewise linear.
fn mapped(points: &[read_fonts::tables::avar::AxisValueMap], value: f64) -> f64 {
    let point = |index: usize| {
        let map = &points[index];
        (
            f64::from(map.from_coordinate().to_f32()),
            f64::from(map.to_coordinate().to_f32()),
        )
    };
    for index in 1..points.len() {
        let ((from0, to0), (from1, to1)) = (point(index - 1), point(index));
        if value <= from1 {
            if from1 == from0 {
                return to1;
            }
            return to0 + (value - from0) / (from1 - from0) * (to1 - to0);
        }
    }
    value
}

struct Faces {
    inter: Face,
    mono: Face,
}

fn faces() -> &'static Faces {
    static FACES: OnceLock<Faces> = OnceLock::new();
    FACES.get_or_init(|| Faces {
        inter: Face::read(INTER),
        mono: Face::read(GEIST_MONO),
    })
}

fn face(font: Font) -> &'static Face {
    let faces = faces();
    if font.mono {
        &faces.mono
    } else {
        &faces.inter
    }
}

/// The weight to set `font` at; the monospace has only one.
fn weight_of(font: Font) -> f64 {
    if font.mono {
        400.0
    } else {
        font.weight
    }
}

/// The unrounded delta of item `index` in `store` at `coords`, with each region scalar in 16.16
/// as Core Text sums them.
fn delta(store: &ItemVariationStore, index: DeltaSetIndex, coords: &[F2Dot14]) -> f64 {
    let Some(Ok(data)) = store.item_variation_data().get(usize::from(index.outer)) else {
        return 0.0;
    };
    let regions = store
        .variation_region_list()
        .expect("a variation store's regions")
        .variation_regions();
    let indexes = data.region_indexes();
    data.delta_set(index.inner)
        .enumerate()
        .map(|(column, value)| {
            let region = regions
                .get(usize::from(indexes[column].get()))
                .expect("a delta's region");
            f64::from(value) * f64::from(region.compute_scalar(coords).to_bits()) / 65536.0
        })
        .sum()
}

/// A face's advances at a weight, in font units, as Core Text computes them.
struct Advances<'a> {
    font: FontRef<'a>,
    coords: &'a [F2Dot14],
    face: &'a Face,
}

impl Advances<'_> {
    fn units(&self, glyph: GlyphId) -> f64 {
        let advance = f64::from(
            self.font
                .hmtx()
                .expect("a face's hmtx")
                .advance(glyph)
                .unwrap_or(0),
        );
        let moved = advance + self.moved(glyph);
        if self.face.fixed_pitch {
            // Core Text stores a fixed-pitch advance in ems, truncated to 16.16.
            (moved / self.face.units_per_em * 65536.0).floor() / 65536.0 * self.face.units_per_em
        } else {
            moved
        }
    }

    /// The `HVAR` delta for `glyph`'s advance at the weight.
    fn moved(&self, glyph: GlyphId) -> f64 {
        if self.coords.iter().all(|coord| *coord == F2Dot14::ZERO) {
            return 0.0;
        }
        let Ok(hvar) = self.font.hvar() else {
            return 0.0;
        };
        let index = match hvar.advance_width_mapping() {
            Some(mapping) => mapping
                .expect("HVAR's advance map")
                .get(glyph.to_u32())
                .expect("a glyph's delta index"),
            None => DeltaSetIndex {
                outer: 0,
                inner: glyph.to_u32() as u16,
            },
        };
        delta(
            &hvar.item_variation_store().expect("HVAR's store"),
            index,
            self.coords,
        )
    }
}

impl FontFuncs for Advances<'_> {
    fn advance_width(&mut self, _builtin: &BuiltinFontFuncs, glyph: GlyphId) -> i32 {
        (self.units(glyph) * SUBUNITS).round() as i32
    }
}

/// One placed glyph, or a run the platform sets (`glyph` is `None` when neither face covers it).
/// In points from the text's start: `x` is the pen position and the glyph is drawn `offset` from
/// it (y up).
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub glyph: Option<u16>,
    /// The first byte it covers; it runs to the next one's `start`.
    pub start: usize,
    pub x: f64,
    pub advance: f64,
    pub offset: (f64, f64),
    /// How many letters it counts for tracking: one, or a platform run's character count.
    pub letters: usize,
}

/// Whether Core Text draws nothing for `character` (Unicode default ignorables: zero-width
/// space, joiners, direction marks, variation selectors).
fn ignored(character: char) -> bool {
    matches!(
        u32::from(character),
        0x00AD | 0x034F | 0x061C | 0x115F..=0x1160 | 0x17B4..=0x17B5 | 0x180B..=0x180F
            | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x206F | 0x3164 | 0xFE00..=0xFE0F
            | 0xFEFF | 0xFFA0 | 0xFFF0..=0xFFF8 | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A
            | 0xE0000..=0xE0FFF
    )
}

type Fallback = dyn Fn(&str, Font) -> f64 + Send + Sync;

static FALLBACK: Mutex<Option<Arc<Fallback>>> = Mutex::new(None);

/// Sets the platform measurer for text neither face covers: its one-line width in the
/// platform's faces at the font's size. Without it such text measures as the missing-glyph box.
pub fn set_fallback(measure: Box<Fallback>) {
    *FALLBACK.relock() = Some(Arc::from(measure));
}

fn fallback() -> Option<Arc<Fallback>> {
    FALLBACK.relock().clone()
}

/// Shaping plan cache key: face, weight, digit setting and script.
type PlanKey = (bool, u64, bool, harfrust::Script);

fn plan(key: PlanKey, make: impl FnOnce() -> ShapePlan) -> Arc<ShapePlan> {
    static PLANS: OnceLock<Mutex<HashMap<PlanKey, Arc<ShapePlan>>>> = OnceLock::new();
    let plans = PLANS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(plan) = plans.relock().get(&key) {
        return plan.clone();
    }
    let made = Arc::new(make());
    plans.relock().insert(key, made.clone());
    made
}

/// Shapes one line of `text` (no line end or tab) in `font`'s face at its weight.
fn shaped(text: &str, font: Font) -> (GlyphBuffer, f64) {
    let face = face(font);
    let weight = weight_of(font);
    let font_ref = face.font();
    let coords = face.coords(weight);
    let instance = ShaperInstance::from_coords(&font_ref, coords.iter().copied());
    let shaper = face
        .data
        .shaper(&font_ref)
        .instance(Some(&instance))
        .build();
    let mut buffer = UnicodeBuffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    let features: Vec<Feature> = if font.tabular {
        vec![Feature::new(Tag::new(b"tnum"), 1, ..)]
    } else {
        Vec::new()
    };
    let key = (font.mono, weight.to_bits(), font.tabular, buffer.script());
    let plan = plan(key, || {
        ShapePlan::new(
            &shaper,
            buffer.direction(),
            Some(buffer.script()),
            buffer.language().as_ref(),
            &features,
        )
    });
    let mut advances = Advances {
        font: font_ref.clone(),
        coords: &coords,
        face,
    };
    let scale = (face.units_per_em * SUBUNITS) as i32;
    let options = ShapeOptions::new()
        .plan(Some(&plan))
        .scale(Some(scale))
        .features(&features)
        .font_funcs(Some(&mut advances));
    let glyphs = shaper.shape(buffer, options);
    (glyphs, font.size / (face.units_per_em * SUBUNITS))
}

/// Whether a face has a glyph for `character`.
fn has(face: &Face, character: char) -> bool {
    face.font()
        .cmap()
        .expect("a face's cmap")
        .map_codepoint(character)
        .is_some()
}

/// Sets `text` on one line in `font`, returning each glyph and its position from 0. Line ends
/// and tabs get no special handling (`typeset` lays those out).
pub fn place(text: &str, font: Font) -> Vec<Placed> {
    let face = face(font);
    let mut placed = Vec::new();
    let mut x = 0.0;
    let mut start = 0;
    // Covered runs are shaped here; uncovered runs go to the platform whole, as Core Text sets
    // a run in a fallback face.
    while start < text.len() {
        let (covered, end) = coverage_run(face, text, start);
        x = match (covered, fallback()) {
            (false, Some(measure)) => {
                let measured = measured_by_platform(&text[start..end], start, x, font, &*measure);
                let after = x + measured.advance;
                placed.push(measured);
                after
            }
            _ => place_shaped(face, text, start..end, font, x, &mut placed),
        };
        start = end;
    }
    placed
}

/// The run from byte `start` that `face` either fully covers or does not cover at all. Returns
/// whether it is covered and the byte where the run ends.
fn coverage_run(face: &Face, text: &str, start: usize) -> (bool, usize) {
    let covered = text[start..]
        .chars()
        .next()
        .is_some_and(|character| has(face, character));
    let end = text[start..]
        .char_indices()
        .find(|&(_, character)| has(face, character) != covered)
        .map_or(text.len(), |(at, _)| start + at);
    (covered, end)
}

/// Places uncovered text from byte `start` at `x` as one run, measured by the platform.
fn measured_by_platform(
    part: &str,
    start: usize,
    x: f64,
    font: Font,
    measure: &Fallback,
) -> Placed {
    Placed {
        glyph: None,
        start,
        x,
        advance: measure(part, font),
        offset: (0.0, 0.0),
        letters: part
            .chars()
            .filter(|&character| !ignored(character))
            .count(),
    }
}

/// Shapes the bytes `run` of `text` in `face` from `x`, appending to `placed`. Returns the pen
/// position after the run.
fn place_shaped(
    face: &Face,
    text: &str,
    run: std::ops::Range<usize>,
    font: Font,
    mut x: f64,
    placed: &mut Vec<Placed>,
) -> f64 {
    let (glyphs, points) = shaped(&text[run.clone()], font);
    let kerning = if font.slanted { 1.0 + SLANT } else { 1.0 };
    let advances = Advances {
        font: face.font(),
        coords: &face.coords(weight_of(font)),
        face,
    };
    for (info, position) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        let cluster = run.start + info.cluster as usize;
        if text[cluster..].chars().next().is_some_and(ignored) {
            continue;
        }
        let nominal = (advances.units(GlyphId::new(info.glyph_id)) * SUBUNITS).round();
        // A glyph shaping hides (a zero-width space) takes no room.
        let advance = if position.x_advance == 0 {
            0.0
        } else {
            (nominal + (f64::from(position.x_advance) - nominal) * kerning) * points
        };
        placed.push(Placed {
            glyph: Some(info.glyph_id as u16),
            start: cluster,
            x,
            advance,
            offset: (
                f64::from(position.x_offset) * points,
                f64::from(position.y_offset) * points,
            ),
            letters: 1,
        });
        x += advance;
    }
    x
}

/// The width of `text` set on one line in `font`.
pub fn width(text: &str, font: Font) -> f64 {
    place(text, font).iter().map(|placed| placed.advance).sum()
}

/// `font`'s ascent and descent in points, both positive, as Core Text reports them.
pub fn metrics(font: Font) -> (f64, f64) {
    let face = face(font);
    (face.ascent * font.size, face.descent * font.size)
}

/// An underline or strikethrough: its top's distance below the baseline (negative is above)
/// and its thickness, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rule {
    pub top: f64,
    pub thickness: f64,
}

/// `font`'s line metrics in points, as Core Text reads them: underline position (its middle,
/// negative below the baseline) and thickness from `post`, and x-height from `OS/2`, each plus
/// `MVAR` at the weight, unrounded. A face without variations keeps them in ems truncated to
/// 16.16, as Core Text does.
fn lines(font: Font) -> (f64, f64, f64) {
    let face = face(font);
    let file = face.font();
    let coords = face.coords(weight_of(font));
    let varied = coords.iter().any(|coord| *coord != F2Dot14::ZERO);
    let mvar = file.mvar().ok();
    let moved = |tag: &[u8; 4]| {
        let Some(mvar) = &mvar else {
            return 0.0;
        };
        let tag = Tag::new(tag);
        mvar.value_records()
            .iter()
            .find(|record| record.value_tag() == tag)
            .map_or(0.0, |record| {
                delta(
                    &mvar
                        .item_variation_store()
                        .expect("MVAR's store")
                        .expect("MVAR's store reads"),
                    DeltaSetIndex {
                        outer: record.delta_set_outer_index(),
                        inner: record.delta_set_inner_index(),
                    },
                    &coords,
                )
            })
    };
    let ems = |units: f64| {
        let ems = units / face.units_per_em;
        if varied {
            ems
        } else {
            (ems * 65536.0).trunc() / 65536.0
        }
    };
    let post = file.post().expect("a face's post");
    let os2 = file.os2().expect("a face's OS/2");
    let position = ems(f64::from(post.underline_position().to_i16()) + moved(b"undo"));
    let thickness = ems(f64::from(post.underline_thickness().to_i16()) + moved(b"unds"));
    let x_height = (f64::from(os2.sx_height().expect("a face's x-height")) + moved(b"xhgt"))
        / face.units_per_em;
    (
        position * font.size,
        thickness * font.size,
        x_height * font.size,
    )
}

/// How thick Core Text draws a `thickness` pt line at `scale` pixels per point: whole device
/// pixels, at least one.
fn drawn_thickness(thickness: f64, scale: f64) -> f64 {
    (thickness * scale).ceil().max(1.0)
}

/// The underline Core Text draws for `font` at `scale` pixels per point: the font's underline
/// with thickness snapped to whole pixels and centred on its position, offset a whole number of
/// pixels from the baseline and never closer to it than its thickness.
pub fn underline(font: Font, scale: f64) -> Rule {
    let (position, thickness, _) = lines(font);
    let drawn = drawn_thickness(thickness, scale);
    let top = (-position * scale - drawn / 2.0 + 0.5).floor().max(drawn);
    Rule {
        top: top / scale,
        thickness: drawn / scale,
    }
}

/// The strikethrough Core Text draws for `font` at `scale` pixels per point: the underline's
/// thickness snapped to whole pixels, centred on half the x-height, a whole number of pixels from
/// the baseline.
pub fn strike(font: Font, scale: f64) -> Rule {
    let (_, thickness, x_height) = lines(font);
    let drawn = drawn_thickness(thickness, scale);
    let top = (-x_height / 2.0 * scale - drawn / 2.0 + 0.5).floor();
    Rule {
        top: top / scale,
        thickness: drawn / scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths Core Text gives (`scripts/text-widths-check.sh` holds every case to it).
    #[test]
    fn a_word_measures_what_core_text_measures() {
        let close = |text: &str, font: Font, core_text: f64| {
            let width = width(text, font);
            assert!(
                (width - core_text).abs() < 1e-5,
                "{text}: {width} against {core_text}"
            );
        };
        close("workspace", Font::sans(13.0, Font::MEDIUM), 68.599084);
        close("workspace", Font::sans(13.0, Font::REGULAR), 67.685059);
        close("workspace", Font::sans(2048.0, Font::MEDIUM), 10806.994141);
        close("AV To", Font::sans(2048.0, Font::REGULAR), 5652.0);
        close("workspace", Font::mono(13.0), 70.198929);
        let mut slanted = Font::sans(13.0, Font::MEDIUM);
        slanted.slanted = true;
        close("workspace", slanted, 68.622011);
        close("1234", Font::sans(13.0, Font::REGULAR).tabular(), 33.71875);
        close("12:30 3x4", Font::sans(13.0, Font::REGULAR), 61.876953);
    }

    /// Core Text's `CTFontGetUnderlinePosition`, `CTFontGetUnderlineThickness` and
    /// `CTFontGetXHeight`, and the lines `CTLineDraw` draws (`scripts/text-widths-check.sh`'s
    /// faces, measured in a bitmap at one and two device pixels a point).
    #[test]
    fn lines_under_and_through_words_sit_where_core_text_draws_them() {
        let close = |font: Font, core_text: (f64, f64, f64)| {
            let (position, thickness, x_height) = lines(font);
            assert!(
                (position - core_text.0).abs() < 1e-9
                    && (thickness - core_text.1).abs() < 1e-9
                    && (x_height - core_text.2).abs() < 1e-9,
                "{font:?}: {:?} against {core_text:?}",
                lines(font)
            );
        };
        close(
            Font::sans(13.0, Font::REGULAR),
            (-2.208984375, 0.888671875, 7.0966796875),
        );
        close(
            Font::sans(13.0, Font::MEDIUM),
            (-2.1564279198646545, 0.9937847852706909, 7.0966796875),
        );
        close(
            Font::sans(13.0, Font::BOLD),
            (-2.0513150095939636, 1.2040106058120728, 7.0966796875),
        );
        close(Font::mono(12.0), (-1.19989013671875, 0.599853515625, 6.36));
        let rule = |top: f64, thickness: f64| Rule { top, thickness };
        // Source monospace at 2x and 1x.
        assert_eq!(underline(Font::mono(12.0), 2.0), rule(1.0, 1.0));
        assert_eq!(strike(Font::mono(12.0), 2.0), rule(-3.5, 1.0));
        assert_eq!(underline(Font::mono(12.0), 1.0), rule(1.0, 1.0));
        assert_eq!(strike(Font::mono(12.0), 1.0), rule(-4.0, 1.0));
        // Card text and page text.
        assert_eq!(
            underline(Font::sans(13.0, Font::REGULAR), 2.0),
            rule(1.5, 1.0)
        );
        assert_eq!(
            strike(Font::sans(13.0, Font::REGULAR), 2.0),
            rule(-4.0, 1.0)
        );
        assert_eq!(
            underline(Font::sans(15.0, Font::REGULAR), 2.0),
            rule(2.0, 1.5)
        );
        assert_eq!(
            strike(Font::sans(15.0, Font::REGULAR), 2.0),
            rule(-5.0, 1.5)
        );
        // A line just over whole pixels stays at least its thickness from the baseline.
        assert_eq!(underline(Font::sans(10.8, Font::BOLD), 2.0), rule(1.5, 1.5));
        assert_eq!(
            underline(Font::sans(28.0, Font::SEMIBOLD), 1.0),
            rule(3.0, 3.0)
        );
        assert_eq!(
            strike(Font::sans(28.0, Font::SEMIBOLD), 1.0),
            rule(-9.0, 3.0)
        );
    }

    #[test]
    fn faces_measure_up_and_down_as_core_text_does() {
        assert_eq!(
            metrics(Font::sans(15.0, Font::REGULAR)),
            (14.53125, 3.6181640625)
        );
        assert_eq!(
            metrics(Font::sans(28.0, Font::SEMIBOLD)),
            (27.125, 6.75390625)
        );
        assert_eq!(
            metrics(Font::mono(13.0)),
            (13.0650634765625, 3.8349761962890625)
        );
    }

    #[test]
    fn an_arrow_typed_as_two_letters_is_one_glyph() {
        let placed = place("->", Font::sans(13.0, Font::REGULAR));
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].start, 0);
    }

    #[test]
    fn words_neither_face_has_are_the_platforms_to_measure() {
        set_fallback(Box::new(|text, font| {
            text.chars().count() as f64 * font.size
        }));
        let placed = place("a◦▪b", Font::sans(10.0, Font::REGULAR));
        assert_eq!(placed.len(), 3);
        assert_eq!(
            (placed[1].glyph, placed[1].start, placed[1].advance),
            (None, 1, 20.0)
        );
        assert_eq!(placed[2].start, "a◦▪".len());
    }
}
