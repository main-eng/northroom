//! Draws the open page's marks (`NrCardMark`, from `page::layout` and `nr_page_overlay`) the way
//! card marks draw, without a card: glyphs, lines, text in the browser setting or the page's own
//! (`Setting::Page`), fills, to-do boxes, outlines and picture files. Marks are in page points;
//! `origin` is the page's top-left in the window.

use gpui::{App, Bounds, Hsla, Pixels, Point, Window, px};
use kit::core::{inset, offset, rect, text};
use kit::draw::{cover, rounded, stroke};
use kit::icons;
use kit::lists::thumbs;
use kit::nr::*;
use kit::text::{Setting, browser_baseline, draw_line, draw_word_lines, metrics, runs_of};
use kit::theme::{Face, Token, color, faded, named};

fn mark_color(mark: &NrCardMark) -> Hsla {
    faded(named(&text(&mark.color)), mark.opacity)
}

/// The borrowed marks at `marks` (`count` of them), as a slice.
pub fn lent(marks: &NrCardMarks) -> &[NrCardMark] {
    if marks.count == 0 {
        return &[];
    }
    // SAFETY: the core lends `count` marks until its next call for the view.
    unsafe { std::slice::from_raw_parts(marks.marks, marks.count) }
}

/// Draws `words` offset to `origin`, with lines shaped by `setting`.
fn words(
    window: &mut Window,
    cx: &mut App,
    words: &NrCardWords,
    origin: Point<Pixels>,
    setting: Setting,
) {
    let (dx, dy) = (f32::from(origin.x), f32::from(origin.y));
    let clip = words.clips.then(|| offset(rect(&words.clip), dx, dy));
    draw_word_lines(
        window,
        cx,
        &runs_of(words, true),
        words.x as f32 + dx,
        words.top as f32 + dy,
        words.width as f32,
        words.wraps,
        words.line_height as f32,
        Face::of(&words.line_font),
        setting,
        clip,
    );
}

/// Draws one page mark with the page's top-left at `origin`.
pub fn paint(window: &mut Window, cx: &mut App, mark: &NrCardMark, origin: Point<Pixels>) {
    let (dx, dy) = (f32::from(origin.x), f32::from(origin.y));
    let frame = offset(rect(&mark.frame), dx, dy);
    let radius = mark.radius as f32;
    match mark.kind {
        NrMarkKind::Glyph => icons::draw(
            window,
            cx,
            icons::named(&text(&mark.text)).expect("a glyph mark names its glyph"),
            frame,
            mark_color(mark),
        ),
        NrMarkKind::Line => draw_line(
            window,
            cx,
            &text(&mark.text),
            Face::of(&mark.font),
            mark_color(mark),
            f32::from(frame.origin.x),
            f32::from(frame.origin.y),
            f32::from(frame.size.height),
            f32::from(frame.size.width),
            None,
        ),
        NrMarkKind::Words => words(window, cx, &mark.words, origin, Setting::Browser),
        NrMarkKind::PageWords => words(window, cx, &mark.words, origin, Setting::Page),
        NrMarkKind::Fill => rounded(window, frame, mark_color(mark), radius),
        NrMarkKind::TodoBox => {
            let (ascent, descent) = metrics(Face::of(&mark.line_font));
            let baseline = browser_baseline(
                mark.line_top as f32 + dy,
                mark.line_height as f32,
                ascent,
                descent,
                window.scale_factor(),
            );
            let boxed = kit::core::frame(
                f32::from(frame.origin.x),
                baseline - mark.frame.y as f32,
                f32::from(frame.size.width),
                f32::from(frame.size.height),
            );
            if mark.stroke > 0.0 {
                stroke(
                    window,
                    inset(boxed, mark.inset as f32, mark.inset as f32),
                    mark_color(mark),
                    radius,
                    mark.stroke as f32,
                );
            } else {
                rounded(window, boxed, mark_color(mark), radius);
            }
        }
        NrMarkKind::Outline => stroke(window, frame, mark_color(mark), radius, mark.stroke as f32),
        NrMarkKind::PictureFile => picture(
            window,
            &text(&mark.text),
            mark.pixels,
            frame,
            named(&text(&mark.color)),
            radius,
        ),
        NrMarkKind::Oval => kit::draw::oval(window, frame, mark_color(mark)),
        other => panic!("the page lays out no {other:?} mark"),
    }
}

/// Draws picture file `id`, at most `pixels` on a side, filling `frame` on `ground` with
/// corners rounded by `radius`.
pub fn picture(
    window: &mut Window,
    id: &str,
    pixels: u32,
    frame: Bounds<Pixels>,
    ground: Hsla,
    radius: f32,
) {
    rounded(window, frame, ground, radius);
    if let Some(image) = thumbs::picture(id, pixels, false) {
        cover(window, &image, frame, gpui::Corners::all(px(radius)));
    }
}

/// The background under a field's text: the slot's own background or the panel's.
pub fn ground(name: Option<&str>) -> Hsla {
    name.map_or(color(Token::Panel), kit::theme::named)
}
