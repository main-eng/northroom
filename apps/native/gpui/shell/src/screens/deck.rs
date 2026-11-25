//! A presentation (`.pptx`), drawn as its slides one under another on the panel colour, each
//! as large as fits the view (ADR-0015). The core reads the file, places the slides and sets
//! their text (`deck::ffi`); this view draws the marks it is given and scrolls. A slide's
//! pictures are files the core keeps in the app's caches, loaded like any picture.
//!
//! The view is read only: nothing on a slide is selected or edited.

use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use gpui::{
    App, Bounds, ContentMask, Context, Corners, EventEmitter, FocusHandle, Hitbox, HitboxBehavior,
    Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, PathBuilder, Pixels, Point, Render, Rgba, ScrollEvent, Styled,
    Window, canvas, point, px,
};
use kit::controls::{Frame, placed};
use kit::core::contains;
use kit::draw::{fill, oval, rounded, stroke};
use kit::lists::scroll::Scroller;
use kit::lists::thumbs;
use kit::motion::now;
use kit::nr::*;
use kit::text::draw_line;
use kit::theme::{Face, Token, color};
use northroom_core::board::card::Font;
use northroom_core::deck::ffi::{deck_marks, deck_page, deck_picture, deck_text, opened_deck};
use northroom_core::deck::layout::{Mark, Page, Words};
use northroom_core::deck::text::{glyph_at, text_between};
use northroom_core::deck::{Colour, Command, Deck as Read, Form, Stroke};
use northroom_core::geometry::Rect;
use std::sync::Arc;

/// The most pixels a slide's picture is loaded at, along its longer side.
const PICTURE_PIXELS_MAX: u32 = 4096;

pub struct Deck {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The deck as the core last read it.
    shown: Option<Arc<Read>>,
    /// Where its slides sit for the view's size.
    page: Option<Page>,
    down: Scroller,
    /// Selected text: the slide, and the glyph the drag began on and the one it reached.
    selection: Option<(usize, usize, usize)>,
    /// Whether a drag is selecting text.
    selecting: bool,
}

/// The key context of a deck view, where ⌘C copies the selected text and ⌘A selects a page's.
const KEYS: &str = "deck";

pub fn bindings() -> Vec<gpui::KeyBinding> {
    use kit::controls::input::{Copy, SelectAll};
    vec![
        gpui::KeyBinding::new("cmd-c", Copy, Some(KEYS)),
        gpui::KeyBinding::new("cmd-a", SelectAll, Some(KEYS)),
    ]
}

/// Where slide `index` of `page` sits in the view, scrolled by `scrolled` (view points).
fn slide_frame(page: &Page, index: usize, scrolled: f64) -> Option<Rect> {
    let slide = page.slides.get(index)?;
    Some(Rect::new(
        slide.x,
        slide.y - scrolled,
        slide.width,
        slide.height,
    ))
}

fn hsla(colour: Colour) -> Hsla {
    Rgba {
        r: f32::from(colour.red) / 255.0,
        g: f32::from(colour.green) / 255.0,
        b: f32::from(colour.blue) / 255.0,
        a: colour.alpha as f32,
    }
    .into()
}

fn face(font: Font) -> Face {
    Face {
        slanted: font.slanted,
        ..Face::sans(font.size as f32, font.weight as f32)
    }
}

/// The corner radius a form's fill or outline is drawn with; `None` for a line, which has
/// neither.
fn radius(form: Form, bounds: Bounds<Pixels>) -> Option<f32> {
    match form {
        Form::Box => Some(0.0),
        Form::Rounded(radius) => Some(radius as f32),
        Form::Oval => Some(f32::from(bounds.size.width.min(bounds.size.height)) / 2.0),
        Form::Line { .. } => None,
    }
}

fn paint_fill(window: &mut Window, bounds: Bounds<Pixels>, form: Form, colour: Colour) {
    match form {
        Form::Box => fill(window, bounds, hsla(colour)),
        Form::Rounded(radius) => rounded(window, bounds, hsla(colour), radius as f32),
        Form::Oval => oval(window, bounds, hsla(colour)),
        Form::Line { .. } => {}
    }
}

fn paint_outline(window: &mut Window, bounds: Bounds<Pixels>, form: Form, line: Stroke) {
    let width = line.width as f32;
    if let Some(radius) = radius(form, bounds) {
        stroke(window, bounds, hsla(line.colour), radius, width);
        return;
    }
    let Form::Line { rising } = form else {
        return;
    };
    let (left, right) = (bounds.origin.x, bounds.origin.x + bounds.size.width);
    let (top, bottom) = (bounds.origin.y, bounds.origin.y + bounds.size.height);
    let (from, to) = if rising {
        (point(left, bottom), point(right, top))
    } else {
        (point(left, top), point(right, bottom))
    };
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(from);
    path.line_to(to);
    if let Ok(path) = path.build() {
        window.paint_path(path, hsla(line.colour));
    }
}

/// Draws picture `index` of the open deck in `bounds`, with `crop` of each edge (left, top,
/// right, bottom, as shares of the picture) cut off. Nothing is drawn while it loads.
fn paint_picture(window: &mut Window, bounds: Bounds<Pixels>, index: usize, crop: [f64; 4]) {
    let longest = f32::from(bounds.size.width.max(bounds.size.height)) * window.scale_factor();
    let pixels = (longest.ceil() as u32)
        .next_power_of_two()
        .min(PICTURE_PIXELS_MAX);
    let image =
        deck_picture(index).and_then(|file| thumbs::picture_file(&file.to_string_lossy(), pixels));
    let Some(image) = image else {
        return;
    };
    let [left, top, right, bottom] = crop.map(|share| share as f32);
    let kept = (
        (1.0 - left - right).max(0.01),
        (1.0 - top - bottom).max(0.01),
    );
    let whole = (bounds.size.width / kept.0, bounds.size.height / kept.1);
    let full = Bounds {
        origin: point(
            bounds.origin.x - whole.0 * left,
            bounds.origin.y - whole.1 * top,
        ),
        size: gpui::size(whole.0, whole.1),
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let _ = window.paint_image(full, full, Corners::default(), image, 0, false);
    });
}

/// Fills a figure's path, its commands in view points from `origin`.
fn paint_figure(window: &mut Window, commands: &[Command], colour: Colour, origin: Point<Pixels>) {
    let at = |x: f64, y: f64| point(px(x as f32) + origin.x, px(y as f32) + origin.y);
    let mut path = PathBuilder::fill();
    for command in commands {
        match *command {
            Command::Move(x, y) => path.move_to(at(x, y)),
            Command::Line(x, y) => path.line_to(at(x, y)),
            Command::Quad(cx, cy, x, y) => path.curve_to(at(x, y), at(cx, cy)),
            Command::Cubic(ax, ay, bx, by, x, y) => {
                path.cubic_bezier_to(at(x, y), at(ax, ay), at(bx, by));
            }
            Command::Close => path.close(),
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, hsla(colour));
    }
}

fn paint_words(window: &mut Window, cx: &mut App, words: &Words, origin: Point<Pixels>) {
    draw_line(
        window,
        cx,
        &words.text,
        face(words.font),
        hsla(words.colour),
        words.x as f32 + f32::from(origin.x),
        words.top as f32 + f32::from(origin.y),
        words.height as f32,
        f32::MAX,
        Some(face(words.line_font)),
    );
}

impl Deck {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut deck = Self {
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            shown: None,
            page: None,
            down: Scroller::default(),
            selection: None,
            selecting: false,
        };
        nr_unlock_forget_asked();
        deck.reload(window, cx);
        deck
    }

    fn place_slides(&mut self) {
        self.page = deck_page(
            f64::from(self.frame.size.width),
            f64::from(self.frame.size.height),
        );
    }

    // MARK: Pointer

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.down.press(event.position, self.frame, now()) {
            cx.notify();
            return;
        }
        // A press on a page starts selecting its text from the glyph under it.
        self.selection = self
            .glyph_under(event.position)
            .map(|(slide, glyph)| (slide, glyph, glyph));
        self.selecting = self.selection.is_some();
        cx.notify();
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.selecting {
            if let Some((slide, anchor, _)) = self.selection {
                let reached = self.glyph_on(slide, event.position);
                if let Some(glyph) = reached {
                    self.selection = Some((slide, anchor, glyph));
                    cx.notify();
                }
            }
            return;
        }
        if self.down.pointer(event.position, self.frame, now()) || self.down.is_held() {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        self.selecting = false;
        if self.down.release(now()) {
            cx.notify();
        }
    }

    /// The view point `at` in the view's own points.
    fn local(&self, at: Point<Pixels>) -> (f64, f64) {
        (
            f64::from(at.x - self.frame.origin.x),
            f64::from(at.y - self.frame.origin.y),
        )
    }

    /// The slide under window point `at`, and the glyph of its text there.
    fn glyph_under(&self, at: Point<Pixels>) -> Option<(usize, usize)> {
        let page = self.page.as_ref()?;
        let (x, y) = self.local(at);
        let scrolled = f64::from(self.down.offset());
        let slide = (0..page.slides.len()).find(|&index| {
            slide_frame(page, index, scrolled).is_some_and(|frame| {
                x >= frame.x
                    && x <= frame.x + frame.width
                    && y >= frame.y
                    && y <= frame.y + frame.height
            })
        })?;
        Some((slide, self.glyph_on(slide, at)?))
    }

    /// The glyph of slide `slide`'s text nearest window point `at`.
    fn glyph_on(&self, slide: usize, at: Point<Pixels>) -> Option<usize> {
        let frame = slide_frame(self.page.as_ref()?, slide, f64::from(self.down.offset()))?;
        glyph_at(&deck_text(slide, frame), self.local(at))
    }

    /// The selected text, as it reads.
    fn selected_text(&self) -> Option<String> {
        let (slide, anchor, reached) = self.selection?;
        let frame = slide_frame(self.page.as_ref()?, slide, 0.0)?;
        Some(text_between(&deck_text(slide, frame), anchor, reached))
    }

    fn copy(&mut self, _: &kit::controls::input::Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text().filter(|text| !text.is_empty()) {
            kit::platform::write_clipboard(&text, cx);
        }
    }

    /// Selects the whole text of the page most in view.
    fn select_all(
        &mut self,
        _: &kit::controls::input::SelectAll,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(page) = self.page.as_ref() else {
            return;
        };
        let middle = f64::from(self.down.offset()) + f64::from(self.frame.size.height) / 2.0;
        let slide = (0..page.slides.len()).min_by(|&a, &b| {
            let distance = |index: usize| {
                (page.slides[index].y + page.slides[index].height / 2.0 - middle).abs()
            };
            distance(a).total_cmp(&distance(b))
        });
        let Some(slide) = slide else {
            return;
        };
        let count = slide_frame(page, slide, 0.0).map_or(0, |frame| deck_text(slide, frame).len());
        if count > 0 {
            self.selection = Some((slide, 0, count - 1));
            cx.notify();
        }
    }

    /// Paints the selection's glyphs on slide `index`, placed at `slide`.
    fn paint_selection(&self, index: usize, slide: Rect, window: &mut Window) {
        let Some((selected, anchor, reached)) =
            self.selection.filter(|selection| selection.0 == index)
        else {
            return;
        };
        let _ = selected;
        let tint = Hsla {
            a: 0.3,
            ..color(Token::Accent)
        };
        let glyphs = deck_text(index, slide);
        for glyph in glyphs
            .iter()
            .skip(anchor.min(reached))
            .take(anchor.max(reached) + 1 - anchor.min(reached))
        {
            fill(window, glyph.frame.at(self.frame.origin), tint);
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if self.down.wheel(event, now()) {
            cx.notify();
        }
    }

    // MARK: Drawing

    /// Draws the slide at `slide` (view points, already scrolled), clipped to itself.
    fn paint_slide(&self, index: usize, slide: Rect, window: &mut Window, cx: &mut App) {
        let origin = self.frame.origin;
        let clip = slide.at(origin).intersect(&self.frame);
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            for mark in deck_marks(index, slide) {
                match mark {
                    Mark::Fill {
                        frame,
                        form,
                        colour,
                    } => paint_fill(window, frame.at(origin), form, colour),
                    Mark::Outline {
                        frame,
                        form,
                        stroke,
                    } => paint_outline(window, frame.at(origin), form, stroke),
                    Mark::Picture {
                        frame,
                        picture,
                        crop,
                    } => paint_picture(window, frame.at(origin), picture, crop),
                    Mark::Words(words) => paint_words(window, cx, &words, origin),
                    Mark::Figure { commands, colour } => {
                        paint_figure(window, &commands, colour, origin);
                    }
                }
            }
            self.paint_selection(index, slide, window);
        });
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        fill(window, self.frame, color(Token::Panel));
        let Some(page) = self.page.clone() else {
            return;
        };
        let now = now();
        let shown = f32::from(self.frame.size.height);
        self.down.set_extent(shown, (page.height as f32).max(shown));
        let scrolled = f64::from(self.down.offset());
        let view = f64::from(shown);
        for (index, slide) in page.slides.iter().enumerate() {
            let top = slide.y - scrolled;
            if top + slide.height < 0.0 || top > view {
                continue;
            }
            let placed = Rect::new(slide.x, top, slide.width, slide.height);
            self.paint_slide(index, placed, window, cx);
        }
        window.with_content_mask(Some(ContentMask { bounds: self.frame }), |window| {
            self.down.paint_bar(window, self.frame, now);
        });
        if self.down.tick(now) {
            window.request_animation_frame();
        }
        self.listen(hitbox, window, cx);
    }

    /// Registers the pointer handlers while the deck is drawn.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let deck = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            if event.button == MouseButton::Left {
                deck.update(cx, |deck, cx| deck.mouse_down(event, window, cx));
            }
        });
        let deck = entity.clone();
        on_bubble(window, move |event: &MouseMoveEvent, _, cx| {
            deck.update(cx, |deck, cx| deck.mouse_move(event, cx));
        });
        let deck = entity.clone();
        on_bubble(window, move |event: &MouseUpEvent, _, cx| {
            if event.button == MouseButton::Left {
                deck.update(cx, |deck, cx| deck.mouse_up(cx));
            }
        });
        let deck = entity;
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            deck.update(cx, |deck, cx| deck.scroll(event, cx));
        });
    }
}

impl EventEmitter<Ask> for Deck {}

impl Screen for Deck {
    /// Reads the open file's deck from the core again. A file that changes on disk keeps its
    /// scroll position, so a slide an agent rewrites is redrawn in place.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // A locked file asks for its password (once; the core keeps track). Deferred, as the
        // first reload runs before the window listens to the new screen.
        let deck = cx.entity();
        cx.defer(move |cx| {
            deck.update(cx, |_, cx| {
                let unlock = nr_prompt_unlock();
                if unlock.effect == NrEffect::Prompt {
                    cx.emit(Ask::Said(kit::lists::rename::Said::of(&unlock)));
                }
            });
        });
        let deck = opened_deck();
        let same = match (&deck, &self.shown) {
            (Some(new), Some(old)) => Arc::ptr_eq(new, old),
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        };
        if same {
            return;
        }
        self.shown = deck;
        self.place_slides();
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            self.place_slides();
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "DeckFileView".to_owned();
        }
    }

    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        let page = self.page.as_ref().filter(|_| contains(&self.frame, at))?;
        let shown = f32::from(self.frame.size.height);
        Some((self.down.offset(), shown, (page.height as f32).max(shown)))
    }
}

impl Render for Deck {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let deck = cx.entity();
        placed("deck", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .key_context(KEYS)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        deck.update(cx, |deck, cx| deck.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
    }
}
