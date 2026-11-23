//! Text measuring, line breaking and drawing. The core sets all text (`northroom_core::text::typeset`,
//! in the app's faces as Core Text would), so the core's layouts, the measurements here and the
//! drawing agree on every platform. Each glyph is painted where the core placed it, and
//! underlines and strikethroughs where the core says Core Text draws them. Only text the app
//! faces have no glyphs for, and text in a platform face (`PlatformFace`), is measured and drawn
//! by GPUI's text system. Lines sit where a browser would put them.

use crate::theme::{self, Face, Token};
use gpui::{
    App, Bounds, ContentMask, Font as PlatformFont, GlyphId, Hsla, LineLayout, Pixels,
    SharedString, TextAlign, TextRun, TextSystem, Window, WindowTextSystem, point, px, size,
};
use northroom_core::board::card::Font;
use northroom_core::board::ffi::{NrCardFont, NrCardRun, NrCardWords};
use northroom_core::ffi::NrText;
use northroom_core::text::typeface::{self, Placed, Rule};
use northroom_core::text::typeset::{self, Stretch};
use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Range;
use std::sync::{Arc, OnceLock};

static TEXT_SYSTEM: OnceLock<Arc<TextSystem>> = OnceLock::new();

/// Layouts a thread's measurer makes before it is reset. GPUI keeps every line it lays out until
/// a frame ends, and a measurer has no frames.
const LAYOUTS_KEPT: usize = 20_000;

struct Measurer {
    system: WindowTextSystem,
    made: usize,
}

thread_local! {
    static MEASURER: RefCell<Option<Measurer>> = const { RefCell::new(None) };
}

/// Gives the measurers GPUI's text system once the application has one.
pub fn install(text_system: Arc<TextSystem>) {
    let _ = TEXT_SYSTEM.set(text_system);
}

fn text_system() -> &'static Arc<TextSystem> {
    TEXT_SYSTEM
        .get()
        .expect("the text system is installed at launch")
}

/// The GPUI font for a core `Face`.
pub fn font(face: Face) -> Font {
    Font {
        size: face.size.into(),
        weight: face.weight.into(),
        mono: face.mono,
        slanted: face.slanted,
        tabular: face.tabular,
    }
}

fn run(len: usize, font: PlatformFont, color: Hsla) -> TextRun {
    TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Lays out `text` on one line in a platform `font` at `size` with this thread's measurer. Only
/// used for text the app faces lack (`measure_fallback`) and for `PlatformFace` text.
fn platform_layout(text: &str, font: PlatformFont, size: f32) -> Arc<LineLayout> {
    MEASURER.with(|measurer| {
        let mut measurer = measurer.borrow_mut();
        if measurer
            .as_ref()
            .is_none_or(|kept| kept.made >= LAYOUTS_KEPT)
        {
            *measurer = Some(Measurer {
                system: WindowTextSystem::new(text_system().clone()),
                made: 0,
            });
        }
        let measurer = measurer.as_mut().expect("made above");
        measurer.made += 1;
        measurer.system.layout_line(
            text,
            px(size),
            &[run(text.len(), font, Hsla::default())],
            None,
        )
    })
}

/// The width of `text` on one line in `face`, measured by the platform, for text the app faces
/// lack (`nr_set_fallback_measure`).
pub extern "C" fn measure_fallback(text: NrText, face: NrCardFont) -> f64 {
    let face = Face::of(&face);
    f64::from(platform_layout(&crate::core::text(&text), face.font(), face.size).width)
}

/// Paints `words` with the platform's text system in `font` at `size`, from `x` on `baseline`.
#[allow(clippy::too_many_arguments)]
fn paint_platform(
    window: &mut Window,
    cx: &mut App,
    words: &str,
    font: PlatformFont,
    size: f32,
    color: Hsla,
    x: f32,
    baseline: f32,
) {
    let line = window.text_system().shape_line(
        SharedString::from(words.to_owned()),
        px(size),
        &[run(words.len(), font, color)],
        None,
    );
    let (ascent, descent) = (line.ascent, line.descent);
    let _ = line.paint(
        point(px(x), px(baseline) - ascent),
        ascent + descent,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

/// A platform face the core has no glyphs for, used where a browser draws its own controls (a
/// video's time in Helvetica, a sound's in the system face). The platform sets this text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlatformFace {
    pub family: &'static str,
    pub size: f32,
}

impl PlatformFace {
    fn font(self) -> PlatformFont {
        gpui::font(self.family)
    }

    /// The width of `words` on one line.
    pub fn width(self, words: &str) -> f32 {
        if words.is_empty() {
            return 0.0;
        }
        f32::from(platform_layout(words, self.font(), self.size).width)
    }

    /// The face's ascent and descent, both positive.
    pub fn metrics(self, cx: &App) -> (f32, f32) {
        let system = cx.text_system();
        let id = system.resolve_font(&self.font());
        (
            f32::from(system.ascent(id, px(self.size))),
            f32::from(system.descent(id, px(self.size))).abs(),
        )
    }

    /// Paints `words` from `x` on `baseline`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        self,
        window: &mut Window,
        cx: &mut App,
        words: &str,
        color: Hsla,
        x: f32,
        baseline: f32,
    ) {
        if !words.is_empty() {
            paint_platform(
                window,
                cx,
                words,
                self.font(),
                self.size,
                color,
                x,
                baseline,
            );
        }
    }
}

/// The width of `text` set on one line in `face`.
pub fn width(text: &str, face: Face) -> f32 {
    typeset::width(text, font(face)) as f32
}

/// The face's ascent and descent as Core Text reports them, both positive.
pub fn metrics(face: Face) -> (f32, f32) {
    let (ascent, descent) = typeface::metrics(font(face));
    (ascent as f32, descent as f32)
}

/// Where a browser puts the baseline in a line box `line_height` tall starting at `top`
/// (`northroom_core::text::baseline::browser`, shared by every shell).
pub fn browser_baseline(top: f32, line_height: f32, ascent: f32, descent: f32, scale: f32) -> f32 {
    northroom_core::text::baseline::browser(
        top.into(),
        line_height.into(),
        ascent.into(),
        descent.into(),
        scale.into(),
    ) as f32
}

/// `text` truncated to `max_width` with a trailing ellipsis, as Core Text truncates, plus its
/// glyphs. Text that fits is borrowed unchanged and shaped once; without tabs or line breaks its
/// width is the sum of glyph advances, as `width` measures it.
fn truncated(text: &str, face: Face, max_width: f32) -> (Cow<'_, str>, Vec<Placed>) {
    let placed = typeface::place(text, font(face));
    let fits = if text.contains(['\n', '\t']) {
        width(text, face) <= max_width
    } else {
        placed
            .iter()
            .fold(0.0, |width, glyph| width + glyph.advance) as f32
            <= max_width
    };
    if fits {
        return (Cow::Borrowed(text), placed);
    }
    let room = f64::from(max_width - width("…", face));
    // The last character boundary where the text before it still fits.
    let (mut before, mut glyph, mut end) = (0.0, 0, 0);
    for (index, _) in text.char_indices().skip(1) {
        while glyph < placed.len() && placed[glyph].start < index {
            before += placed[glyph].advance;
            glyph += 1;
        }
        if before > room {
            break;
        }
        end = index;
    }
    let shown = format!("{}…", text[..end].trim_end());
    let placed = typeface::place(&shown, font(face));
    (Cow::Owned(shown), placed)
}

/// Draws one line of `text` as a browser lays it out: the face's height (or `line_face`'s, for
/// text sharing a baseline with other text) centred in a line box `line_height` tall at `top`,
/// truncated with an ellipsis past `max_width`.
#[allow(clippy::too_many_arguments)]
pub fn draw_line(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    face: Face,
    color: Hsla,
    x: f32,
    top: f32,
    line_height: f32,
    max_width: f32,
    line_face: Option<Face>,
) {
    if text.is_empty() {
        return;
    }
    let (shown, placed) = truncated(text, face, max_width);
    let (ascent, descent) = metrics(line_face.unwrap_or(face));
    let baseline = browser_baseline(top, line_height, ascent, descent, window.scale_factor());
    paint_glyphs(
        window,
        cx,
        &shown,
        shown.len(),
        &placed,
        face,
        color,
        x,
        baseline,
    );
}

/// How far a glyph may ink past its advance on either side (a slanted stroke, a wide bowl). A
/// glyph within this distance of a fade is drawn through it.
const GLYPH_REACH: f32 = 2.0;

/// Draws one line of `text` like `draw_line`, without truncating. With `fade` (window x range),
/// the text is opaque up to its start and fades linearly to nothing at its end, per device
/// pixel, as a browser tab fades an overlong title.
#[allow(clippy::too_many_arguments)]
pub fn draw_fading_line(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    face: Face,
    color: Hsla,
    x: f32,
    top: f32,
    line_height: f32,
    fade: Option<Range<f32>>,
) {
    let (ascent, descent) = metrics(face);
    let baseline = browser_baseline(top, line_height, ascent, descent, window.scale_factor());
    let placed = typeface::place(text, font(face));
    // Tall enough for any glyph's ink above and below.
    let rows = (top - line_height)..(top + 2.0 * line_height);
    for (index, glyph) in placed.iter().enumerate() {
        let until = placed.get(index + 1).map_or(text.len(), |next| next.start);
        let reach = (x + glyph.x as f32 - GLYPH_REACH)
            ..(x + (glyph.x + glyph.advance) as f32 + GLYPH_REACH);
        let paint = |window: &mut Window, cx: &mut App, shade: Hsla| {
            let one = std::slice::from_ref(glyph);
            paint_glyphs(window, cx, text, until, one, face, shade, x, baseline);
        };
        let Some(fade) = fade.clone().filter(|fade| reach.end > fade.start) else {
            paint(window, cx, color);
            continue;
        };
        for (columns, opacity) in fade_strips(reach, fade, 1.0 / window.scale_factor()) {
            let strip = Bounds {
                origin: point(px(columns.start), px(rows.start)),
                size: size(px(columns.end - columns.start), px(rows.end - rows.start)),
            };
            window.with_content_mask(Some(ContentMask { bounds: strip }), |window| {
                paint(window, cx, theme::faded(color, f64::from(opacity)));
            });
        }
    }
}

/// The strips a glyph spanning `reach` is drawn in across `fade`, each with its opacity: one at
/// full opacity before the fade, then strips `step` wide counted from the fade's start, each at
/// the fade's opacity at its centre. Nothing is drawn past the fade's end.
fn fade_strips(reach: Range<f32>, fade: Range<f32>, step: f32) -> Vec<(Range<f32>, f32)> {
    let mut strips = Vec::new();
    if reach.start < fade.start {
        strips.push((reach.start..fade.start, 1.0));
    }
    let length = fade.end - fade.start;
    let last = reach.end.min(fade.end);
    // The first fade strip the glyph reaches.
    let mut count = ((reach.start - fade.start) / step).floor().max(0.0);
    while fade.start + count * step < last {
        let from = fade.start + count * step;
        let to = (from + step).min(fade.end);
        let opacity = 1.0 - ((from + to) / 2.0 - fade.start) / length;
        strips.push((from..to, opacity));
        count += 1.0;
    }
    strips
}

/// Paints `glyphs` of `text` (up to byte `end`) in `face` at the core's positions from `x`, on
/// `baseline`. GPUI draws text the app faces lack in a platform face.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_glyphs(
    window: &mut Window,
    cx: &mut App,
    text: &str,
    end: usize,
    glyphs: &[Placed],
    face: Face,
    color: Hsla,
    x: f32,
    baseline: f32,
) {
    let font_id = window.text_system().resolve_font(&face.font());
    for (index, placed) in glyphs.iter().enumerate() {
        let left = x + placed.x as f32;
        match placed.glyph {
            Some(glyph) => {
                let origin = point(
                    px(left + placed.offset.0 as f32),
                    px(baseline - placed.offset.1 as f32),
                );
                // A glyph that can't be drawn leaves a gap, like a missing glyph.
                let _ = window.paint_glyph(
                    origin,
                    font_id,
                    GlyphId(glyph.into()),
                    px(face.size),
                    color,
                );
            }
            None => {
                let until = glyphs.get(index + 1).map_or(end, |next| next.start);
                paint_platform(
                    window,
                    cx,
                    &text[placed.start..until],
                    face.font(),
                    face.size,
                    color,
                    left,
                    baseline,
                );
            }
        }
    }
}

/// Fills `rule` relative to `baseline`, `width` wide from `x`.
fn paint_rule(window: &mut Window, rule: Rule, color: Hsla, x: f32, baseline: f32, width: f32) {
    if width <= 0.0 {
        return;
    }
    window.paint_quad(gpui::fill(
        Bounds {
            origin: point(px(x), px(baseline + rule.top as f32)),
            size: size(px(width), px(rule.thickness as f32)),
        },
        color,
    ));
}

/// Draws an underline or strikethrough `width` wide from `x`, where Core Text draws them in
/// `face` (`typeface::underline`, `typeface::strike`). Underlines use `underline`'s colour;
/// strikethroughs use the text colour.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_marks(
    window: &mut Window,
    face: Face,
    color: Hsla,
    x: f32,
    baseline: f32,
    width: f32,
    underline: Option<Hsla>,
    strike: bool,
) {
    let scale = f64::from(window.scale_factor());
    if let Some(under) = underline {
        paint_rule(
            window,
            typeface::underline(font(face), scale),
            under,
            x,
            baseline,
            width,
        );
    }
    if strike {
        paint_rule(
            window,
            typeface::strike(font(face), scale),
            color,
            x,
            baseline,
            width,
        );
    }
}

/// The width of a set line excluding trailing spaces, which Core Text doesn't underline or
/// strike through.
fn inked_width(runs: &[Run], line: &SetLine) -> f32 {
    line.pieces
        .iter()
        .flat_map(|piece| {
            let text = &runs[piece.run].text;
            piece.glyphs.iter().enumerate().map(move |(index, glyph)| {
                let until = piece
                    .glyphs
                    .get(index + 1)
                    .map_or(piece.end, |next| next.start);
                (piece, glyph, &text[glyph.start..until])
            })
        })
        .filter(|(_, _, words)| !words.chars().all(char::is_whitespace))
        .map(|(piece, glyph, _)| piece.x + (glyph.x + glyph.advance) as f32)
        .fold(0.0, f32::max)
}

/// A run of text in one face (`NrCardRun`), as the shell sets it.
pub struct Run {
    pub text: String,
    pub face: Face,
    pub color: Hsla,
    pub underline: Option<Hsla>,
    pub strike: bool,
    pub code: bool,
    /// Blank space in place of text: after the preceding letter, or before the text when first.
    pub space: f32,
    /// Space after each letter (the open page's title uses negative tracking).
    pub tracking: f32,
}

impl Run {
    /// A run from the core's description. With `colors` false, colours are left out (for
    /// measuring). Tabs become `tab` (spaces per tab stop); an empty `tab` keeps real tabs at Core
    /// Text's stops.
    pub fn of(run: &NrCardRun, colors: bool, tab: &str) -> Self {
        let named = |name: &str, opacity: f64| theme::faded(theme::named(name), opacity);
        let underline = crate::core::text(&run.underline);
        let text = crate::core::text(&run.text);
        Self {
            text: if tab.is_empty() {
                text
            } else {
                text.replace('\t', tab)
            },
            face: Face::of(&run.font),
            color: if colors {
                named(&crate::core::text(&run.color), run.opacity)
            } else {
                Hsla::default()
            },
            underline: (colors && !underline.is_empty())
                .then(|| named(&underline, run.underline_opacity)),
            strike: run.strike,
            code: run.code,
            space: run.space as f32,
            tracking: run.tracking as f32,
        }
    }
}

/// The part of one run on a set line, starting at `x`, with glyph positions relative to `x`.
pub struct Piece {
    pub run: usize,
    pub start: usize,
    pub end: usize,
    pub x: f32,
    pub width: f32,
    pub glyphs: Vec<Placed>,
}

impl Piece {
    /// The x of the caret position before byte `at` of the run's text, from the line start.
    pub fn x_of(&self, at: usize) -> f32 {
        self.x
            + self
                .glyphs
                .iter()
                .filter(|glyph| glyph.start < at)
                .map(|glyph| glyph.advance)
                .sum::<f64>() as f32
    }
}

pub struct SetLine {
    pub pieces: Vec<Piece>,
    pub width: f32,
}

/// Sets `runs` into lines `width` wide, wrapping only if `wraps`, as the core does
/// (`typeset::set`). Tabs go to Core Text's stops.
pub fn set(runs: &[Run], width: f32, wraps: bool) -> Vec<SetLine> {
    let stretches: Vec<Stretch> = runs
        .iter()
        .map(|run| Stretch {
            text: &run.text,
            font: font(run.face),
            space: run.space.into(),
            tracking: run.tracking.into(),
        })
        .collect();
    typeset::set(&stretches, width.into(), wraps, 0)
        .into_iter()
        .map(|line| SetLine {
            width: line.width as f32,
            pieces: line
                .pieces
                .into_iter()
                .map(|piece| Piece {
                    run: piece.stretch,
                    start: piece.start,
                    end: piece.end,
                    x: piece.x as f32,
                    width: piece.width as f32,
                    glyphs: piece.glyphs,
                })
                .collect(),
        })
        .collect()
}

/// The runs in a core text description, with tabs as the requested number of spaces or at Core
/// Text's stops.
pub fn runs_of(words: &NrCardWords, colors: bool) -> Vec<Run> {
    let tab = " ".repeat(words.tab_spaces as usize);
    if words.run_count == 0 {
        return Vec::new();
    }
    // SAFETY: the core passes `run_count` runs at `runs`.
    let runs = unsafe { std::slice::from_raw_parts(words.runs, words.run_count) };
    runs.iter().map(|run| Run::of(run, colors, &tab)).collect()
}

/// How a line finds its baseline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    /// Browser rule: the line face's height centred in the line, snapped to device pixels
    /// (`browser_baseline`).
    Browser,
    /// The open page's rule (`NrMarkKind::PageWords`): Core Text's fixed line height in points
    /// (`nr_page_baseline`).
    Page,
}

/// Draws text as the core set it (`NrCardWords`), with browser baselines, rounded backgrounds
/// behind inline code, strikethroughs and link underlines.
pub fn draw_words(window: &mut Window, cx: &mut App, words: &NrCardWords) {
    draw_set_words(window, cx, words, Setting::Browser);
}

/// Draws text as the core set it, with baselines placed by `setting`.
pub fn draw_set_words(window: &mut Window, cx: &mut App, words: &NrCardWords, setting: Setting) {
    let clip = words.clips.then(|| crate::core::rect(&words.clip));
    draw_word_lines(
        window,
        cx,
        &runs_of(words, true),
        words.x as f32,
        words.top as f32,
        words.width as f32,
        words.wraps,
        words.line_height as f32,
        Face::of(&words.line_font),
        setting,
        clip,
    );
}

/// Draws `runs` set into lines `width` wide from (`x`, `top`), wrapping only if `wraps`. Each
/// line is exactly `line_height` tall with `line_face` placed by `setting`, plus inline code
/// backgrounds, strikethroughs and link underlines. Card text and the open page, static or being
/// edited, all draw here, so lines don't shift when editing starts. Returns the line count.
#[allow(clippy::too_many_arguments)]
pub fn draw_word_lines(
    window: &mut Window,
    cx: &mut App,
    runs: &[Run],
    x: f32,
    top: f32,
    width: f32,
    wraps: bool,
    line_height: f32,
    line_face: Face,
    setting: Setting,
    clip: Option<Bounds<Pixels>>,
) -> usize {
    let lines = set(runs, width, wraps);
    draw_set_lines(
        window,
        cx,
        runs,
        &lines,
        x,
        top,
        line_height,
        line_face,
        setting,
        clip,
    );
    lines.len()
}

/// Draws already-set `lines` of `runs` (`set`) from (`x`, `top`), like `draw_word_lines`.
#[allow(clippy::too_many_arguments)]
pub fn draw_set_lines(
    window: &mut Window,
    cx: &mut App,
    runs: &[Run],
    lines: &[SetLine],
    x: f32,
    top: f32,
    line_height: f32,
    line_face: Face,
    setting: Setting,
    clip: Option<Bounds<Pixels>>,
) {
    let (ascent, descent) = metrics(line_face);
    let scale = window.scale_factor();
    let page_baseline =
        northroom_core::ffi::nr_page_baseline(line_height.into(), ascent.into(), descent.into())
            as f32;
    let clip = clip.map(|bounds| ContentMask { bounds });
    window.with_content_mask(clip, |window| {
        for (index, line) in lines.iter().enumerate() {
            let line_top = top + index as f32 * line_height;
            let baseline = match setting {
                Setting::Browser => browser_baseline(line_top, line_height, ascent, descent, scale),
                Setting::Page => line_top + page_baseline,
            };
            paint_code_ground(window, runs, line, x, baseline);
            paint_line_pieces(window, cx, runs, line, x, baseline);
        }
    });
}

/// Draws the background behind inline code on one set `line`: 2 pt above and below the code's
/// height, spanning its padding on both sides.
fn paint_code_ground(window: &mut Window, runs: &[Run], line: &SetLine, x: f32, baseline: f32) {
    let mut ground: Option<Bounds<Pixels>> = None;
    for piece in &line.pieces {
        let run = &runs[piece.run];
        if !run.code || piece.end == piece.start {
            if piece.end > piece.start {
                fill_ground(&mut ground, window);
            }
            continue;
        }
        let padding = runs.get(piece.run + 1).map_or(0.0, |next| next.space);
        let (run_ascent, run_descent) = metrics(run.face);
        let left = x + piece.x;
        let part = Bounds {
            origin: point(px(left - padding), px(baseline - run_ascent - 2.0)),
            size: size(
                px(piece.width + padding * 2.0),
                px(run_ascent + run_descent + 4.0),
            ),
        };
        ground = Some(ground.map_or(part, |whole| whole.union(&part)));
    }
    fill_ground(&mut ground, window);
}

/// Fills and clears the accumulated background behind a stretch of inline code, if any.
fn fill_ground(ground: &mut Option<Bounds<Pixels>>, window: &mut Window) {
    if let Some(whole) = ground.take() {
        window.paint_quad(gpui::fill(whole, theme::color(Token::Muted)).corner_radii(px(6.0)));
    }
}

/// Draws one set `line` on `baseline`, with its strikethroughs and underlines.
fn paint_line_pieces(
    window: &mut Window,
    cx: &mut App,
    runs: &[Run],
    line: &SetLine,
    x: f32,
    baseline: f32,
) {
    let inked = inked_width(runs, line);
    for piece in &line.pieces {
        if piece.end == piece.start {
            continue;
        }
        let run = &runs[piece.run];
        let left = x + piece.x;
        paint_glyphs(
            window,
            cx,
            &run.text,
            piece.end,
            &piece.glyphs,
            run.face,
            run.color,
            left,
            baseline,
        );
        paint_marks(
            window,
            run.face,
            run.color,
            left,
            baseline,
            piece.width.min(inked - piece.x),
            run.underline,
            run.strike,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::fade_strips;

    #[test]
    fn a_glyph_before_a_fade_is_whole_and_one_past_it_is_not_drawn() {
        // A fade from 100 to 104, 1 pt per strip.
        assert_eq!(
            fade_strips(90.0..100.0, 100.0..104.0, 1.0),
            [(90.0..100.0, 1.0)]
        );
        assert_eq!(fade_strips(104.0..112.0, 100.0..104.0, 1.0), []);
    }

    #[test]
    fn a_glyph_across_a_fade_is_drawn_whole_before_it_and_weaker_strip_by_strip_through_it() {
        assert_eq!(
            fade_strips(98.0..110.0, 100.0..104.0, 1.0),
            [
                (98.0..100.0, 1.0),
                (100.0..101.0, 0.875),
                (101.0..102.0, 0.625),
                (102.0..103.0, 0.375),
                (103.0..104.0, 0.125),
            ]
        );
    }

    #[test]
    fn two_glyphs_share_the_strips_of_a_fade_where_they_meet() {
        // Strips count from the fade's start, not the glyph, so a glyph starting inside the fade
        // shares its neighbour's strip. 0.5 pt strips, as on a 2x display.
        assert_eq!(
            fade_strips(101.25..103.0, 100.0..104.0, 0.5),
            [
                (101.0..101.5, 0.6875),
                (101.5..102.0, 0.5625),
                (102.0..102.5, 0.4375),
                (102.5..103.0, 0.3125),
            ]
        );
    }
}
