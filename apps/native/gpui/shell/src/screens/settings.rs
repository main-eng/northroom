//! The Settings appearance page. The core describes it (`app_ffi::settings_page`) and lays it
//! out from its controls' sizes (`app_ffi::settings_window`), both as the core's own types. Each
//! control is a `kit::controls` view placed where the core puts it. Input goes to the core as an
//! action (`nr_settings_act`), and the page rebuilds from the core's new description.
//!
//! A colour swatch opens kit's picker beside it. The page follows the colour while picking and
//! rebuilds once the picker closes. The page column scrolls on `kit::lists::scroll`, and the
//! controls are placed again as it moves.

use super::{Ask, Screen, on_bubble};
use crate::script::probe::Probe;
use gpui::{
    AnyView, App, AppContext, Bounds, Context, Entity, EventEmitter, Hitbox, HitboxBehavior,
    IntoElement, ParentElement, Pixels, Point, Render, ScrollEvent, Styled, Subscription, Window,
    canvas, div,
};
use kit::controls::picker::{self, ColourChanged, OpenPicker, Popover, Swatch};
use kit::controls::select::{self, Choice, Opened, Select, Swatch as Dot};
use kit::controls::slider::{self, Moved, Slider};
use kit::controls::theme_choice::{Picked, ThemeChoice};
use kit::controls::{Drawn, Frame};
use kit::core::{c, inset, offset};
use kit::draw::{fill, rounded, stroke};
use kit::lists::scroll::Scroller;
use kit::menu::Run;
use kit::motion::now;
use kit::nr::*;
use kit::text::{draw_line, width};
use kit::theme::{self, Face, Token, color};
use northroom_core::app_ffi::{settings_page, settings_window};
use northroom_core::dialog::layout::Size;
use northroom_core::settings::page::layout::{PageLayout, Window as Laid};
use northroom_core::settings::page::{Control, Page};
use std::rc::Rc;

// MARK: Controls

/// A built control: a kit view, or text the page draws itself.
enum View {
    Words(String),
    Select(Entity<Select>),
    Theme(Entity<ThemeChoice>),
    Swatch(Entity<Swatch>),
    Slider(Entity<Slider>),
}

impl View {
    fn view(&self) -> Option<AnyView> {
        Some(match self {
            Self::Words(_) => return None,
            Self::Select(view) => view.clone().into(),
            Self::Theme(view) => view.clone().into(),
            Self::Swatch(view) => view.clone().into(),
            Self::Slider(view) => view.clone().into(),
        })
    }

    /// Puts the control at `at` in the window.
    fn place(&self, at: Bounds<Pixels>, cx: &mut App) {
        match self {
            Self::Words(_) => {}
            Self::Select(view) => view.update(cx, |select, cx| select.place(at, cx)),
            Self::Theme(view) => view.update(cx, |choice, cx| choice.place(at, cx)),
            Self::Swatch(view) => view.update(cx, |swatch, cx| swatch.place(at, cx)),
            Self::Slider(view) => view.update(cx, |slider, cx| slider.place(at, cx)),
        }
    }
}

struct Built {
    view: View,
    /// The control's size, which the core lays the page out from.
    size: (f32, f32),
    /// Where the core put it in the window, with the page scrolled to the top.
    laid: Bounds<Pixels>,
    /// Where it is now, at the current scroll offset.
    at: Bounds<Pixels>,
    /// Text that sits in one of the page's own text lines.
    page_line: bool,
}

pub struct Settings {
    /// The page as described when its controls were built.
    page: Page,
    built: Vec<Built>,
    frame: Bounds<Pixels>,
    /// Where the core laid the page out.
    laid: Option<Laid>,
    /// The column the page scrolls in, in the window.
    column: Bounds<Pixels>,
    /// The height of the column's scrolling content.
    document: f32,
    scroller: Scroller,
    /// The scroll offset when the controls were last placed.
    placed_offset: f32,
    /// The open colour picker.
    picker: Option<Entity<Popover>>,
    picker_events: Vec<Subscription>,
    events: Vec<Subscription>,
}

impl Settings {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut settings = Self {
            page: Page::default(),
            built: Vec::new(),
            frame: Bounds::default(),
            laid: None,
            column: Bounds::default(),
            document: 0.0,
            scroller: Scroller::default(),
            placed_offset: 0.0,
            picker: None,
            picker_events: Vec::new(),
            events: Vec::new(),
        };
        settings.reload(window, cx);
        settings
    }

    /// Sends input on control `action` to the core and carries out any platform work it returns.
    /// Then rebuilds the page (`rebuild`), or only takes its new state, as dragging does.
    fn act(
        &mut self,
        action: &str,
        value: &str,
        rebuild: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (named, value) = (c(action), c(value));
        // SAFETY: NUL-terminated strings.
        match unsafe { nr_settings_act(named.as_ptr(), value.as_ptr()) } {
            NrSettingsActed::Nothing => {}
            NrSettingsActed::ThemeChanged => cx.emit(Ask::ThemeChanged),
        }
        if rebuild {
            self.reload(window, cx);
        } else {
            self.page = settings_page();
        }
        cx.emit(Ask::Changed);
    }

    /// Builds the view for `control`, wired to the core, and returns its size.
    fn build(
        &mut self,
        control: Control,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (View, (f32, f32)) {
        match control {
            Control::Words { text } => {
                let size = (width(&text, Face::sans(14.0, 400.0)), 20.0);
                (View::Words(text), size)
            }
            Control::Select {
                action,
                enabled,
                choices,
                selected,
                width,
            } => {
                let choices = choices
                    .into_iter()
                    .map(|choice| Choice {
                        id: choice.id,
                        name: choice.name,
                        swatch: choice.swatch.as_deref().map(|swatch| Dot {
                            color: theme::named(swatch),
                        }),
                    })
                    .collect();
                let select = cx.new(|_| {
                    let mut select = Select::new(choices, selected);
                    select.enabled = enabled;
                    select
                });
                self.events.push(cx.subscribe_in(
                    &select,
                    window,
                    move |_, select, _: &Opened, _, cx| {
                        let settings = cx.entity();
                        let action = action.clone();
                        let run: Run = Rc::new(move |entry, window, cx| {
                            let id = entry.id.clone();
                            settings.update(cx, |settings, cx| {
                                settings.act(&action, &id, true, window, cx)
                            });
                        });
                        let select = select.read(cx);
                        cx.emit(Ask::Choices {
                            menu: select.menu(),
                            at: select.menu_origin(),
                            run,
                        });
                    },
                ));
                (View::Select(select), (width as f32, select::HEIGHT))
            }
            Control::Theme {
                action,
                labels,
                selected,
            } => {
                let choice = ThemeChoice::new(labels, selected);
                let size = choice.size();
                let choice = cx.new(|_| choice);
                self.events.push(cx.subscribe_in(
                    &choice,
                    window,
                    move |settings, _, picked: &Picked, window, cx| {
                        settings.act(&action, &picked.0.to_string(), true, window, cx);
                    },
                ));
                (View::Theme(choice), size)
            }
            Control::Color { action, hex } => {
                let swatch = Swatch::new(&hex);
                let size = swatch.size();
                let swatch = cx.new(|_| swatch);
                self.events.push(cx.subscribe_in(
                    &swatch,
                    window,
                    move |settings, swatch, open: &OpenPicker, window, cx| {
                        settings.open_picker(swatch.clone(), &action, open, window, cx)
                    },
                ));
                (View::Swatch(swatch), size)
            }
            Control::Slider { action, value } => {
                let slider = cx.new(|_| Slider::new(value));
                self.events.push(cx.subscribe_in(
                    &slider,
                    window,
                    move |settings, _, moved: &Moved, window, cx| {
                        settings.act(&action, &moved.0.to_string(), false, window, cx);
                    },
                ));
                (View::Slider(slider), slider::SIZE)
            }
        }
    }

    /// Lays the page out from its controls' sizes and places them.
    fn lay_out(&mut self, cx: &mut Context<Self>) {
        let sizes: Vec<Size> = self
            .built
            .iter()
            .map(|built| Size {
                width: f64::from(built.size.0),
                height: f64::from(built.size.1),
            })
            .collect();
        let content = self.frame;
        let laid = settings_window(
            &self.page,
            f64::from(content.size.width),
            f64::from(content.size.height),
            &sizes,
        );
        let (x0, y0) = (f32::from(content.origin.x), f32::from(content.origin.y));
        self.column = offset(laid.column.bounds(), x0, y0);
        self.document = laid.document.height as f32;
        self.scroller
            .set_extent(f32::from(self.column.size.height), self.document);
        let pane = self.pane(&laid, 0.0);
        let (px0, py0) = (f32::from(pane.origin.x), f32::from(pane.origin.y));
        for (built, placed) in self.built.iter_mut().zip(&laid.page.controls) {
            built.laid = offset(placed.frame.bounds(), px0, py0);
            built.page_line = placed.page_line;
        }
        self.laid = Some(laid);
        self.place_controls(cx);
    }

    /// Puts each control where the core laid it, shifted up by the scroll offset.
    fn place_controls(&mut self, cx: &mut Context<Self>) {
        let scrolled = self.scroller.offset();
        self.placed_offset = scrolled;
        for built in &mut self.built {
            built.at = offset(built.laid, 0.0, -scrolled);
            built.view.place(built.at, cx);
        }
        cx.notify();
    }

    /// The page pane in the window per `laid`, scrolled down by `scrolled`.
    fn pane(&self, laid: &Laid, scrolled: f32) -> Bounds<Pixels> {
        let document = laid.document.bounds();
        offset(
            laid.page_frame.bounds(),
            f32::from(self.column.origin.x) + f32::from(document.origin.x),
            f32::from(self.column.origin.y) + f32::from(document.origin.y) - scrolled,
        )
    }

    // MARK: The colour picker

    /// Opens the picker for `swatch`, whose colour is set by `action`.
    fn open_picker(
        &mut self,
        swatch: Entity<Swatch>,
        action: &str,
        open: &OpenPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let popover = cx.new(|cx| Popover::open(open.swatch, &open.hex, window, cx));
        let picker = popover.read(cx).picker.clone();
        let changing = action.to_owned();
        self.picker_events = vec![
            // The page follows the colour while picking and keeps its controls.
            cx.subscribe_in(
                &picker,
                window,
                move |settings, _, changed: &ColourChanged, window, cx| {
                    let hex = swatch.update(cx, |swatch, cx| {
                        swatch.set(&changed.0);
                        cx.notify();
                        swatch.hex.clone()
                    });
                    settings.act(&changing, &hex, false, window, cx);
                },
            ),
            cx.subscribe_in(&popover, window, |_, _, _: &picker::Closed, window, cx| {
                // Lay the page out again once the typed value is sent.
                let settings = cx.entity();
                window.defer(cx, move |window, cx| {
                    settings.update(cx, |settings, cx| {
                        settings.picker = None;
                        settings.picker_events.clear();
                        settings.page = Page::default();
                        settings.reload(window, cx);
                    })
                });
            }),
        ];
        self.picker = Some(popover);
        cx.notify();
    }

    // MARK: Scrolling

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if self.scroller.wheel(event, now()) {
            self.place_controls(cx);
        }
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let Some(laid) = &self.laid else {
            return;
        };
        let moment = now();
        let pane = self.pane(laid, self.placed_offset);
        let page = &laid.page;
        let column = self.column;
        window.with_content_mask(Some(gpui::ContentMask { bounds: column }), |window| {
            paint_page(window, cx, pane, page);
            self.paint_built_words(window, cx);
        });
        // The scrollbar fade animates frame by frame.
        if self.scroller.tick(moment) {
            window.request_animation_frame();
        }
        let settings = cx.entity();
        let over = hitbox.clone();
        on_bubble(window, move |event: &ScrollEvent, window, cx| {
            if over.should_handle_scroll(window) && kit::core::contains(&column, event.position) {
                settings.update(cx, |settings, cx| settings.scroll(event, cx));
            }
        });
    }

    /// Paints the controls the page draws as its own text.
    fn paint_built_words(&self, window: &mut Window, cx: &mut App) {
        for built in &self.built {
            if let View::Words(words) = &built.view {
                paint_words(window, cx, words, built.at, built.page_line);
            }
        }
    }

    /// Paints the overlay scrollbar above the controls.
    fn paint_bar(&self, window: &mut Window) {
        self.scroller.paint_bar(window, self.column, now());
    }
}

/// Paints the page background: its boxes, dividers and text lines.
fn paint_page(window: &mut Window, cx: &mut App, pane: Bounds<Pixels>, page: &PageLayout) {
    let (x0, y0) = (f32::from(pane.origin.x), f32::from(pane.origin.y));
    let border = color(Token::Border);
    for placed in &page.boxes {
        let at = offset(placed.bounds(), x0, y0);
        rounded(window, at, color(Token::Card), 6.0);
        stroke(window, inset(at, 0.5, 0.5), border, 5.5, 1.0);
    }
    for placed in &page.dividers {
        fill(window, offset(placed.bounds(), x0, y0), border);
    }
    for line in &page.lines {
        line.draw(window, cx, pane.origin, None);
    }
}

/// Text between controls: muted 14 px type, or 16 px type on a 24 px line when it sits in one
/// of the page's own text lines.
fn paint_words(
    window: &mut Window,
    cx: &mut App,
    words: &str,
    at: Bounds<Pixels>,
    page_line: bool,
) {
    let (line_height, line_face) = if page_line {
        (24.0, Some(Face::sans(16.0, 400.0)))
    } else {
        (20.0, None)
    };
    draw_line(
        window,
        cx,
        words,
        Face::sans(14.0, 400.0),
        color(Token::MutedForeground),
        f32::from(at.origin.x),
        f32::from(at.origin.y),
        line_height,
        f32::MAX,
        line_face,
    );
}

impl EventEmitter<Ask> for Settings {}

impl Screen for Settings {
    /// Rebuilds the page if it changed.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = settings_page();
        if page == self.page && !self.built.is_empty() {
            return;
        }
        let controls: Vec<Control> = page.controls().cloned().collect();
        self.page = page;
        self.events.clear();
        self.built = Vec::new();
        for control in controls {
            let (view, size) = self.build(control, window, cx);
            self.built.push(Built {
                view,
                size,
                laid: Bounds::default(),
                at: Bounds::default(),
                page_line: false,
            });
        }
        self.lay_out(cx);
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            self.lay_out(cx);
        }
    }

    fn probe(&self, probe: &mut Probe, _: &Window, _: &App) {
        probe.settings = true;
    }

    /// The page's scroll offset, visible height and content height; `None` outside the column.
    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        kit::core::contains(&self.column, at).then(|| {
            (
                self.scroller.offset(),
                f32::from(self.column.size.height),
                self.document,
            )
        })
    }
}

impl Render for Settings {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The page scrolled since the controls were placed.
        if self.scroller.offset() != self.placed_offset {
            self.place_controls(cx);
        }
        let settings = cx.entity();
        let bar = cx.entity();
        let column = self.column;
        let controls: Vec<AnyView> = self
            .built
            .iter()
            .filter_map(|built| built.view.view())
            .collect();
        kit::controls::placed("settings", cx.entity_id(), self.frame)
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        settings.update(cx, |settings, cx| settings.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
            // Controls are placed in window points and clipped to the page column.
            .child(
                div()
                    .absolute()
                    .left(column.origin.x - self.frame.origin.x)
                    .top(column.origin.y - self.frame.origin.y)
                    .w(column.size.width)
                    .h(column.size.height)
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(-column.origin.x)
                            .top(-column.origin.y)
                            .children(controls),
                    ),
            )
            // The overlay scrollbar over the controls, and the colour picker over everything.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, cx| bar.read(cx).paint_bar(window),
                )
                .absolute()
                .size_full(),
            )
            .children(self.picker.clone())
    }
}
