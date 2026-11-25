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
use northroom_core::deck::ffi::{deck_marks, deck_page, deck_picture, opened_deck};
use northroom_core::deck::layout::{Mark, Page, Words};
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
        };
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

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if self.down.press(event.position, self.frame, now()) {
            cx.notify();
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if self.down.pointer(event.position, self.frame, now()) || self.down.is_held() {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.down.release(now()) {
            cx.notify();
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
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Left {
                deck.update(cx, |deck, cx| deck.mouse_down(event, cx));
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
