//! One folder's column. Rows come from the core (`nr_column_row`), inset 4 pt top and bottom.
//! The selection bar uses the accent while the column is active (focused or renaming) and grey
//! otherwise. Folders show a caret, the right edge has a hairline, and an empty folder shows
//! muted text. Only visible rows are drawn.

use super::chip;
use super::rename::{Ended, Rename, Renamed, Said};
use super::rows::{Key, Rows};
use super::scroll::{Route, Scroller};
use super::{
    DriveCommands, Item, drop_ring, file_icon, rows_of, selected, selection_bar, voiceover,
};
use crate::core::{c, contains, frame, text, zeroed};
use crate::draw::fill;
use crate::icons::{self, Glyph};
use crate::keys::held;
use crate::motion::now;
use crate::nr::NrDriveEdit;
use crate::text::{draw_line, width};
use crate::theme::{self, Face, Token, color, mixed};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, Hitbox,
    HitboxBehavior, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollEvent, Styled, Window, canvas, div,
    prelude::*,
};
use northroom_core::app_ffi::nr_column_empty_text;
use northroom_core::ffi::{
    NrFileRow, nr_column_drop, nr_column_drop_place, nr_column_open, nr_column_row,
    nr_column_row_count, nr_column_row_layout, nr_drag_starts,
};
use northroom_core::menus::ffi::nr_folder_drop;
use northroom_core::window::ffi::{NrDropPlace, nr_rows_context};
use std::rc::Rc;

/// Row inset from the column's top and bottom.
const INSET: f32 = 4.0;

/// What a column asks of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The core's folders, selection or open file changed; the window should refresh.
    Changed,
    /// A right click on `row` (-1 for the space below the rows) at `at` in the window. The core
    /// has already updated the selection; `about` are the rows the menu acts on.
    RowMenu {
        row: i64,
        about: Vec<usize>,
        at: Point<Pixels>,
    },
    /// ← or →: column `n` takes the keys.
    Column(usize),
    /// The core's reply to a rename, drop or drive command, to be carried out.
    Said(Said),
}

pub struct Column {
    /// The core's table number for this column.
    pub index: usize,
    focus: FocusHandle,
    items: Vec<Item>,
    row_height: f32,
    /// The column's frame in the window, including its hairline edge.
    frame: Bounds<Pixels>,
    table: Rows,
    pub scroller: Scroller,
    /// Vertical gestures belong to the column; the rest go to the row of columns.
    route: Route,
    drop_target: Option<usize>,
    rename: Option<Rename>,
    /// The file the last single press chose. A second press completing a double click applies
    /// to this file, even if the columns have moved its row since.
    chosen: Option<Chosen>,
}

/// The file a press chose and where in the window it landed.
struct Chosen {
    id: String,
    at: Point<Pixels>,
}

impl EventEmitter<Event> for Column {}

impl Focusable for Column {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Column {
    pub fn new(index: usize, cx: &mut Context<Self>) -> Self {
        let mut column = Self {
            index,
            focus: cx.focus_handle(),
            items: Vec::new(),
            row_height: nr_column_row_layout().height as f32,
            frame: Bounds::default(),
            table: Rows::new(index as i64),
            scroller: Scroller::default(),
            route: Route::new(false),
            drop_target: None,
            rename: None,
            chosen: None,
        };
        column.reload();
        column
    }

    /// Reloads the column's rows.
    pub fn reload(&mut self) {
        self.items = (0..nr_column_row_count(self.index))
            .filter_map(|row| {
                let mut data: NrFileRow = zeroed();
                // SAFETY: a row to fill.
                unsafe { nr_column_row(self.index, row, &mut data) }.then(|| Item::of(&data))
            })
            .collect();
    }

    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    /// Focuses the column.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// Whether the column is active: focused, or renaming a row.
    fn worked_in(&self, window: &Window) -> bool {
        self.focus.is_focused(window) || self.rename.is_some()
    }

    /// The scroller's frame: the column without its hairline edge.
    fn scroller_frame(&self) -> Bounds<Pixels> {
        let mut bounds = self.frame;
        bounds.size.width -= gpui::px(1.0);
        bounds
    }

    /// The row under `at`: `Some(-1)` for the space under or above the rows, `None` outside.
    pub fn row_at(&self, at: Point<Pixels>) -> Option<i64> {
        let view = self.scroller_frame();
        if !contains(&view, at) {
            return None;
        }
        let y = f32::from(at.y - view.origin.y) + self.scroller.offset() - INSET;
        let row = (y / self.row_height).floor();
        Some(if row >= 0.0 && (row as usize) < self.items.len() {
            row as i64
        } else {
            -1
        })
    }

    fn frame_of(&self, row: usize) -> Bounds<Pixels> {
        let view = self.scroller_frame();
        let top = f32::from(view.origin.y) + INSET + row as f32 * self.row_height
            - self.scroller.offset();
        frame(
            f32::from(view.origin.x),
            top,
            f32::from(view.size.width),
            self.row_height,
        )
    }

    /// Where the row named `title` is drawn in the window, for scripted checks.
    pub fn row_frame(&self, title: &str) -> Option<Bounds<Pixels>> {
        let row = self.items.iter().position(|item| item.title == title)?;
        Some(self.frame_of(row))
    }

    fn reveal(&mut self, row: usize) {
        let top = INSET + row as f32 * self.row_height;
        self.scroller
            .reveal(top - INSET, top + self.row_height + INSET);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(Event::Changed);
        cx.notify();
    }

    fn open(&mut self, row: usize, cx: &mut Context<Self>) {
        if self.rename.is_none() {
            nr_column_open(self.index, row);
            self.changed(cx);
        }
    }

    // MARK: Rename

    /// Opens the rename field over `row`'s name.
    pub fn begin_rename(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.items.get(row) else {
            return;
        };
        if self.rename.is_some() {
            return;
        }
        let (id, title) = (item.id.clone(), item.title.clone());
        self.reveal(row);
        let renamed = Renamed::Row {
            column: self.index,
            row,
        };
        self.rename = Some(Rename::begin(
            &id,
            renamed,
            &title,
            window,
            cx,
            |column: &mut Self, ended, window, cx| column.end_rename(ended, window, cx),
        ));
        cx.notify();
    }

    fn end_rename(&mut self, ended: Ended, window: &mut Window, cx: &mut Context<Self>) {
        self.rename = None;
        window.focus(&self.focus, cx);
        if let Ended::Committed(said) = ended {
            self.reload();
            cx.emit(Event::Said(said));
        }
        self.changed(cx);
    }

    fn rename_frame(&self, id: &str) -> Option<Bounds<Pixels>> {
        let row = self.items.iter().position(|item| item.id == id)?;
        let bar = self.frame_of(row);
        let x = nr_column_row_layout().rename as f32;
        Some(frame(
            f32::from(bar.origin.x) + x,
            f32::from(bar.origin.y) + 4.0,
            f32::from(bar.size.width) - x - 28.0,
            20.0,
        ))
    }

    // MARK: Pointer

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = event.position;
        if self.scroller.press(at, self.scroller_frame(), now()) {
            cx.notify();
            return;
        }
        let Some(row) = self.row_at(at) else { return };
        window.focus(&self.focus, cx);
        if super::asks_for_menu(event) {
            self.table.forget();
            let about = rows_of(&nr_rows_context(self.index as i64, row));
            cx.emit(Event::RowMenu { row, about, at });
            self.changed(cx);
            return;
        }
        if event.button == MouseButton::Left {
            if event.click_count == 1 {
                self.chosen = usize::try_from(row)
                    .ok()
                    .and_then(|row| self.items.get(row))
                    .map(|item| Chosen {
                        id: item.id.clone(),
                        at,
                    });
            }
            self.table
                .down(row, at, held(&event.modifiers), event.click_count);
        }
        cx.notify();
    }

    /// Handles a press anywhere in the window before the view under it; `over` is true when it
    /// is over the column. The first press of a double click selects its row, which can shift
    /// the columns (a folder opens, a file preview appears, they scroll), so the second press
    /// may land elsewhere. It still applies to the first press's row; returns true when the
    /// column took it. A second press far from the first belongs to some other press.
    fn second_press(
        &mut self,
        event: &MouseDownEvent,
        over: bool,
        scale: f32,
        cx: &mut Context<Self>,
    ) -> bool {
        let chosen = self.chosen.take();
        if event.button != MouseButton::Left || event.click_count != 2 {
            return false;
        }
        let Some(row) = chosen
            .filter(|chosen| {
                let travelled = event.position - chosen.at;
                !nr_drag_starts(
                    f64::from(travelled.x),
                    f64::from(travelled.y),
                    f64::from(scale),
                )
            })
            .and_then(|chosen| self.items.iter().position(|item| item.id == chosen.id))
        else {
            return false;
        };
        // The row is still under the pointer, so handle it as a normal press.
        if over && self.row_at(event.position) == Some(row as i64) {
            return false;
        }
        self.table
            .down(row as i64, event.position, held(&event.modifiers), 2);
        cx.notify();
        true
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = event.position;
        let view = self.scroller_frame();
        if self.scroller.is_held() {
            self.scroller.pointer(at, view, now());
            cx.notify();
            return;
        }
        if chip::follow(at) {
            self.drag_over(over.then(|| self.row_at(at)).flatten());
            // Outside the window, the files continue as a platform drag.
            chip::carry_out(at, window, cx);
            window.refresh();
            return;
        }
        if event.pressed_button == Some(MouseButton::Left)
            && let Some(carried) = self.table.moved(at, window.scale_factor())
        {
            if !carried.is_empty() {
                let picked: Vec<&Item> = carried
                    .iter()
                    .filter_map(|&row| self.items.get(row))
                    .collect();
                let ids = picked.iter().map(|item| item.id.clone()).collect();
                let titles: Vec<String> = picked.iter().map(|item| item.title.clone()).collect();
                chip::begin(self.index as i64, ids, &titles, at);
                self.drag_over(self.row_at(at));
                window.refresh();
            }
            self.changed(cx);
            return;
        }
        if self.scroller.pointer(at, view, now()) {
            cx.notify();
        }
    }

    fn drop_place(&self, ids: &[String], row: i64) -> NrDropPlace {
        let ids = c(&ids.join("\n"));
        // SAFETY: a NUL-terminated list.
        unsafe { nr_column_drop_place(ids.as_ptr(), self.index, row) }
    }

    /// Rings the folder a drag over `row` would drop into (`None` when not over the column).
    fn drag_over(&mut self, row: Option<i64>) {
        self.drop_target = row.and_then(|row| {
            let ids = chip::carried()?;
            matches!(
                self.drop_place(&ids, row),
                NrDropPlace::Folder | NrDropPlace::Terminal
            )
            .then_some(row as usize)
        });
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, over: bool, cx: &mut Context<Self>) {
        if self.scroller.release(now()) {
            cx.notify();
            return;
        }
        if let Some(drag) = chip::current() {
            self.drop_target = None;
            if let Some(row) = over.then(|| self.row_at(event.position)).flatten() {
                chip::end();
                self.drop_on(&drag.ids, row, cx);
            }
            cx.notify();
            return;
        }
        if let Some(open) = self.table.up() {
            match open {
                Some(row) => self.open(row, cx),
                None => self.changed(cx),
            }
        }
    }

    /// Drops the dragged items `ids` on `row`: into a terminal row's session, else into the
    /// column, wherever `drop_place` allows. Nothing happens where it allows none.
    fn drop_on(&mut self, ids: &[String], row: i64, cx: &mut Context<Self>) {
        let place = self.drop_place(ids, row);
        if place == NrDropPlace::Nowhere {
            return;
        }
        let ids = c(&ids.join("\n"));
        let said = if place == NrDropPlace::Terminal {
            let terminal = c(&self.items[row as usize].id);
            // SAFETY: NUL-terminated strings.
            Said::of(&unsafe { nr_folder_drop(self.index, ids.as_ptr(), terminal.as_ptr()) })
        } else {
            // SAFETY: a NUL-terminated list.
            Said::refused(&unsafe { nr_column_drop(ids.as_ptr(), self.index, row) })
        };
        self.reload();
        cx.emit(Event::Said(said));
        self.changed(cx);
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let now = now();
        let mut moved = false;
        for own in self.route.own(event) {
            moved |= self.scroller.wheel(&own, now);
        }
        if moved {
            cx.notify();
        }
    }

    /// The scroll offset, visible height and inset content height when `at` is over the column
    /// (`ev:scrolled`).
    pub fn scrolled_at(&self, at: Point<Pixels>) -> Option<(f32, f32, f32)> {
        let view = self.scroller_frame();
        contains(&view, at).then(|| {
            (
                self.scroller.offset(),
                f32::from(view.size.height),
                self.items.len() as f32 * self.row_height + 2.0 * INSET,
            )
        })
    }

    // MARK: Keys

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        if event.keystroke.key == "escape" && chip::end().is_some() {
            self.drop_target = None;
            window.refresh();
            cx.stop_propagation();
            return;
        }
        match self.table.key(&event.keystroke) {
            Key::PassOn => return,
            Key::Selection(row) => {
                if let Some(row) = row {
                    self.reveal(row);
                }
                self.changed(cx);
            }
            Key::Reload => self.changed(cx),
            Key::Open(row) => self.open(row, cx),
            Key::OpenParent => {}
            Key::Rename(row) => self.begin_rename(row, window, cx),
            // Trash the selection without asking; ⌘Z undoes it.
            Key::Trash => {
                let said = super::edit_files(NrDriveEdit::Trash, self.index as i64);
                cx.emit(Event::Said(said));
                self.changed(cx);
            }
            Key::Column(column) => cx.emit(Event::Column(column)),
        }
        cx.stop_propagation();
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        let view = self.scroller_frame();
        self.scroller.set_extent(
            f32::from(view.size.height),
            self.items.len() as f32 * self.row_height + 2.0 * INSET,
        );
        let (left, top) = (
            f32::from(self.frame.origin.x),
            f32::from(self.frame.origin.y),
        );
        let (column_width, height) = (
            f32::from(self.frame.size.width),
            f32::from(self.frame.size.height),
        );
        // Right edge: a hairline at 6% of the text colour.
        fill(
            window,
            frame(left + column_width - 1.0, top, 1.0, height),
            mixed(Token::Foreground, 0.06),
        );
        if self.items.is_empty() {
            // Empty folder: muted 12 px text, centred, 24 px below the row inset.
            let face = Face::sans(12.0, 400.0);
            let words = text(&nr_column_empty_text());
            let x = left + 4.0 + (column_width - 9.0 - width(&words, face)) / 2.0;
            draw_line(
                window,
                cx,
                &words,
                face,
                color(Token::MutedForeground),
                x,
                top + 28.0,
                16.0,
                column_width,
                None,
            );
        }
        let worked_in = self.worked_in(window);
        window.with_content_mask(Some(gpui::ContentMask { bounds: view }), |window| {
            self.paint_rows(window, cx, view, worked_in);
            self.scroller.paint_bar(window, view, now);
        });
        if self.scroller.tick(now) {
            window.request_animation_frame();
        }
        self.listen(hitbox, window, cx);
    }

    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        Self::listen_for_buttons(&entity, hitbox, window);
        let (column, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = over.is_hovered(window);
                column.update(cx, |column, cx| {
                    column.mouse_move(event, hovered, window, cx)
                });
            }
        });
        let (column, over) = (entity, hitbox.clone());
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                column.update(cx, |column, cx| column.scroll(event, cx));
            }
        });
    }

    /// Routes to the column a press over its `hitbox`, the second press of a double click
    /// begun on one of its rows, and any left-button release.
    fn listen_for_buttons(entity: &Entity<Self>, hitbox: &Hitbox, window: &mut Window) {
        let (column, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            let hovered = over.is_hovered(window);
            match phase {
                DispatchPhase::Capture => {
                    let scale = window.scale_factor();
                    if column.update(cx, |column, cx| {
                        column.second_press(event, hovered, scale, cx)
                    }) {
                        cx.stop_propagation();
                    }
                }
                DispatchPhase::Bubble if hovered => {
                    column.update(cx, |column, cx| column.mouse_down(event, window, cx));
                }
                DispatchPhase::Bubble => {}
            }
        });
        let (column, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                let hovered = over.is_hovered(window);
                column.update(cx, |column, cx| column.mouse_up(event, hovered, cx));
            }
        });
    }

    fn paint_rows(&self, window: &mut Window, cx: &mut App, view: Bounds<Pixels>, worked_in: bool) {
        let height = self.row_height;
        let (left, rows_width) = (f32::from(view.origin.x), f32::from(view.size.width));
        let scrolled = self.scroller.offset() - INSET;
        let first = (scrolled / height).floor().max(0.0) as usize;
        let last = (((scrolled + f32::from(view.size.height)) / height)
            .ceil()
            .max(0.0) as usize)
            .min(self.items.len());
        let selected = selected(self.index as i64);
        for (row, item) in self.items.iter().enumerate().take(last).skip(first) {
            let top = f32::from(view.origin.y) + row as f32 * height - scrolled;
            let bar = frame(left + 4.0, top, rows_width - 9.0, height);
            let is_selected = selected.binary_search(&row).is_ok();
            if is_selected {
                let above = row > 0 && selected.binary_search(&(row - 1)).is_ok();
                let below = selected.binary_search(&(row + 1)).is_ok();
                selection_bar(window, bar, worked_in, above, below);
            }
            if self.drop_target == Some(row) {
                drop_ring(window, bar, is_selected);
            }
            let ink = if is_selected && worked_in {
                color(Token::AccentForeground)
            } else {
                color(Token::Foreground)
            };
            self.paint_item(window, cx, item, (left, top), ink);
        }
    }

    /// Draws one row at (`left`, `top`) in `ink`: icon, name (left to the rename field while
    /// renaming) and a folder's caret.
    fn paint_item(
        &self,
        window: &mut Window,
        cx: &mut App,
        item: &Item,
        (left, top): (f32, f32),
        ink: Hsla,
    ) {
        let layout = nr_column_row_layout();
        // Same icon and 24 px slot as the list's rows.
        file_icon(
            window,
            cx,
            item,
            frame(left + layout.icon as f32, top + 2.0, 24.0, 24.0),
            ink,
        );
        let (caret, name) = (layout.caret as f32, layout.name as f32);
        let trailing = if item.holds_files {
            caret - 6.0
        } else {
            caret + 12.0
        };
        let renaming = self
            .rename
            .as_ref()
            .is_some_and(|rename| rename.id == item.id);
        if !renaming {
            draw_line(
                window,
                cx,
                &item.title,
                Face::sans(12.0, 400.0),
                ink,
                left + name,
                top + 6.0,
                16.0,
                trailing - name,
                None,
            );
        }
        if item.holds_files {
            icons::draw(
                window,
                cx,
                Glyph::CaretRight,
                frame(left + caret, top + 8.0, 12.0, 12.0),
                theme::faded(ink, 0.6),
            );
        }
    }
}

impl Column {
    /// Every row's accessibility node, built only while assistive tech reads the window.
    fn heard(&self, window: &Window) -> Vec<voiceover::HeardRow> {
        if !window.is_a11y_active() {
            return Vec::new();
        }
        let chosen = selected(self.index as i64);
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| voiceover::HeardRow {
                name: item.title.clone(),
                selected: chosen.contains(&index),
                frame: self.frame_of(index),
            })
            .collect()
    }
}

impl Render for Column {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(rename) = &self.rename
            && let Some(at) = self.rename_frame(&rename.id)
        {
            rename.place(at, cx);
        }
        let column = cx.entity();
        let (commanded, index) = (column.clone(), self.index);
        let frame = self.frame;
        let field = self.rename.as_ref().map(|rename| rename.field.clone());
        let heard = self.heard(window);
        div()
            .id(("kit-column", cx.entity_id()))
            .absolute()
            .left(frame.origin.x)
            .top(frame.origin.y)
            .w(frame.size.width)
            .h(frame.size.height)
            .role(voiceover::SCROLL_AREA)
            .a11y_synthetic_children(voiceover::scroll_area())
            // Copy files dropped from outside into the column's folder.
            .on_drop(cx.listener(|column, dropped: &gpui::ExternalPaths, _, cx| {
                cx.emit(Event::Said(super::import(column.index, dropped)));
                column.changed(cx);
            }))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        column.update(cx, |column, cx| column.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
            // The table sits in a scroll area, as `NSTableView` does. It holds focus and takes
            // the drive's commands on the selection.
            .child(
                div()
                    .id("table")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .role(voiceover::TABLE)
                    .aria_row_count(self.items.len())
                    .track_focus(&self.focus)
                    .key_context(super::CONTEXT)
                    .take_drive_commands(
                        &self.focus,
                        move || index as i64,
                        Rc::new(move |said, _, cx| {
                            commanded.update(cx, |column, cx| {
                                cx.emit(Event::Said(said));
                                column.changed(cx);
                            })
                        }),
                    )
                    .on_key_down(cx.listener(Self::key_down))
                    .children(voiceover::rows(heard, frame.origin))
                    .children(voiceover::column("name"))
                    .children(field.map(|field| {
                        div()
                            .absolute()
                            .left(-frame.origin.x)
                            .top(-frame.origin.y)
                            .child(field)
                    })),
            )
    }
}
