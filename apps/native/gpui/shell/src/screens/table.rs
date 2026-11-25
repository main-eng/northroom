//! A spreadsheet file: delimited text (`.csv`, `.tsv`) as one sheet, an Excel or OpenDocument
//! workbook (`.xlsx`, `.xlsm`, `.ods`) as its visible sheets, or a SQLite database as its tables
//! (rows read as they scroll into view). The grid keeps column headings and row numbers fixed
//! while it scrolls, with a line below and right of every cell.
//!
//! A workbook with several sheets shows them as flat tabs in a bar under the grid, on the muted
//! ground with a line to the right of each. The open tab sits on the background with no top line
//! and a 2 px primary-colour line below; the others sit under a line and highlight on hover. The
//! core lays out the table, cells, text and tabs (`table_view`, `table_grid`, `nr_table_tab_at`).

use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use gpui::{
    App, Bounds, Context, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, ScrollDelta, ScrollEvent, Styled, Window, canvas, point, px,
};
use kit::controls::placed;
use kit::controls::{Drawn, Frame};
use kit::core::{contains, frame};
use kit::draw::fill;
use kit::lists::scroll::Scroller;
use kit::motion::now;
use kit::nr::*;
use kit::theme::{Token, color, mixed};
use northroom_core::sheets::layout::{self, Tabs, View};

pub struct Table {
    frame: Bounds<Pixels>,
    focus: FocusHandle,
    /// The table as the core last gave it.
    shown: Option<layout::Table>,
    sheet: usize,
    /// The tab under the pointer.
    hovered: Option<usize>,
    placed: Option<View>,
    down: Scroller,
    across: Scroller,
}

impl Table {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut table = Self {
            frame: Bounds::default(),
            focus: cx.focus_handle(),
            shown: None,
            sheet: 0,
            hovered: None,
            placed: None,
            down: Scroller::default(),
            across: Scroller::default(),
        };
        table.reload(window, cx);
        table
    }

    /// Lays the view out for its size and the hovered tab, as the core places it.
    fn place_parts(&mut self) {
        self.placed = table_view(
            self.sheet,
            f64::from(self.frame.size.width),
            f64::from(self.frame.size.height),
            self.hovered,
        );
    }

    /// Shows one sheet, scrolled to its own top-left.
    fn show(&mut self, sheet: usize, cx: &mut Context<Self>) {
        self.sheet = sheet;
        self.down = Scroller::default();
        self.across = Scroller::default();
        self.place_parts();
        cx.notify();
    }

    /// The grid scroller's window frame, while the grid shows.
    fn grid_frame(&self) -> Option<Bounds<Pixels>> {
        let placed = self.placed.as_ref()?;
        placed
            .grid_shown
            .then(|| placed.scroll.at(self.frame.origin))
    }

    /// The sheet bar's window frame, if there is one.
    fn bar_frame(&self) -> Option<Bounds<Pixels>> {
        let tabs = self.placed.as_ref()?.tabs.as_ref()?;
        Some(tabs.frame.at(self.frame.origin))
    }

    fn tab_at(&self, at: Point<Pixels>) -> Option<usize> {
        let bar = self.bar_frame().filter(|bar| contains(bar, at))?;
        let found = nr_table_tab_at(
            self.sheet,
            f64::from(at.x - bar.origin.x),
            f64::from(at.y - bar.origin.y),
        );
        usize::try_from(found).ok()
    }

    // MARK: Pointer

    fn mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some(grid) = self.grid_frame()
            && self.down.press(event.position, grid, now())
        {
            cx.notify();
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let now = now();
        if let Some(grid) = self.grid_frame() {
            if self.down.is_held() {
                self.down.pointer(event.position, grid, now);
                cx.notify();
                return;
            }
            if self.down.pointer(event.position, grid, now) {
                cx.notify();
            }
        }
        let hovered = self.tab_at(event.position);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.place_parts();
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.down.release(now()) {
            cx.notify();
            return;
        }
        // A press on a sheet tab; the core keeps the choice.
        if let Some(tab) = self.tab_at(event.position) {
            nr_table_choose_sheet(tab);
            self.show(nr_table_sheet(), cx);
        }
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let now = now();
        let mut moved = self.down.wheel(event, now);
        // Horizontal travel goes to the horizontal scroller as its own scroll axis.
        let across = ScrollEvent {
            delta: match event.delta {
                ScrollDelta::Lines(delta) => ScrollDelta::Lines(point(0.0, delta.x)),
                ScrollDelta::Pixels(delta) => ScrollDelta::Pixels(point(px(0.0), delta.x)),
            },
            ..event.clone()
        };
        moved |= self.across.wheel(&across, now);
        if moved {
            cx.notify();
        }
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let Some(placed) = self.placed.clone() else {
            return;
        };
        let origin = self.frame.origin;
        fill(window, self.frame, color(placed.ground));
        if let Some(message) = &placed.message {
            message.draw(window, cx, origin, None);
        }
        if let Some(grid) = self.grid_frame() {
            let now = now();
            // A database table can be taller than an `f32` holds at point precision.
            let shown = f64::from(f32::from(grid.size.height));
            self.down
                .set_long_extent(shown, placed.grid_height.max(shown));
            self.across.set_extent(
                f32::from(grid.size.width),
                (placed.grid_width as f32).max(f32::from(grid.size.width)),
            );
            window.with_content_mask(Some(gpui::ContentMask { bounds: grid }), |window| {
                self.paint_grid(grid, window, cx);
                self.down.paint_bar(window, grid, now);
                self.across.paint_bar_across(window, grid, now);
            });
            if self.down.tick(now) | self.across.tick(now) {
                window.request_animation_frame();
            }
        }
        if let (Some(tabs), Some(bar)) = (&placed.tabs, self.bar_frame()) {
            paint_tabs(tabs, bar, window, cx);
        }
        self.listen(hitbox, window, cx);
    }

    /// The visible part of the grid, as the core places its cells. Positions are relative to the
    /// view's top-left, so a row millions down draws as precisely as the first.
    fn paint_grid(&self, grid: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        fill(window, grid, color(Token::Background));
        let cells = table_grid(
            self.sheet,
            self.across.long_offset(),
            self.down.long_offset(),
            f64::from(grid.size.width),
            f64::from(grid.size.height),
        );
        let corner = grid.origin;
        let line = color(Token::Border);
        for cell in &cells {
            let bounds = cell.frame.at(corner);
            if cell.heading {
                fill(window, bounds, color(Token::Muted));
            }
            let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            fill(window, frame(left, top + height - 1.0, width, 1.0), line);
            fill(window, frame(left + width - 1.0, top, 1.0, height), line);
            if let Some(words) = &cell.words {
                words.draw(window, cx, corner, None);
            }
        }
    }

    /// Registers the pointer handlers while the table is drawn.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let table = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, _, cx| {
            if event.button == MouseButton::Left {
                table.update(cx, |table, cx| table.mouse_down(event, cx));
            }
        });
        let table = entity.clone();
        on_bubble(window, move |event: &MouseMoveEvent, _, cx| {
            table.update(cx, |table, cx| table.mouse_move(event, cx));
        });
        let (table, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseUpEvent, window, cx| {
            if event.button == MouseButton::Left
                && (over.is_hovered(window) || table.read(cx).down.is_held())
            {
                table.update(cx, |table, cx| table.mouse_up(event, cx));
            }
        });
        let table = entity;
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            table.update(cx, |table, cx| table.scroll(event, cx));
        });
    }
}

/// Paints the sheet bar at `bar` in the window.
fn paint_tabs(tabs: &Tabs, bar: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    fill(window, bar, color(Token::Muted));
    let line = color(Token::Border);
    let origin = bar.origin;
    for tab in &tabs.tabs {
        let at = tab.frame.at(origin);
        let (left, top) = (f32::from(at.origin.x), f32::from(at.origin.y));
        let (width, height) = (f32::from(at.size.width), f32::from(at.size.height));
        if tab.open {
            fill(window, at, color(Token::Background));
            fill(
                window,
                frame(left, top + height - 2.0, width - 1.0, 2.0),
                color(Token::Primary),
            );
        } else {
            if tab.hovered {
                fill(window, at, mixed(Token::Foreground, 0.08));
            }
            fill(
                window,
                frame(left, f32::from(bar.origin.y), width, 1.0),
                line,
            );
        }
        let side = if tab.open { height - 2.0 } else { height };
        fill(
            window,
            frame(left + width - 1.0, f32::from(bar.origin.y), 1.0, side),
            line,
        );
        tab.words.draw(window, cx, origin, None);
    }
    // The rest of the bar, under its line.
    fill(window, tabs.rest.at(origin), line);
}

impl EventEmitter<Ask> for Table {}

impl Screen for Table {
    /// Reads the open file's table from the core again. The core keeps the chosen sheet while the
    /// file stays the same (`nr_table_sheet`). A file that changes on disk keeps its scroll
    /// position while its sheet exists, so rows written to a database appear in place.
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let table = opened_table();
        if table == self.shown {
            return;
        }
        self.shown = table;
        let sheet = nr_table_sheet();
        if sheet == self.sheet {
            self.place_parts();
            cx.notify();
        } else {
            self.show(sheet, cx);
        }
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            self.place_parts();
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "TableFileView".to_owned();
        }
    }

    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        let grid = self.grid_frame().filter(|grid| contains(grid, at))?;
        let placed = self.placed.as_ref()?;
        Some((
            self.down.offset(),
            f32::from(grid.size.height),
            (placed.grid_height as f32).max(f32::from(grid.size.height)),
        ))
    }
}

impl Render for Table {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let table = cx.entity();
        placed("table", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        table.update(cx, |table, cx| table.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
    }
}
