//! The folder as a list: a sortable header, then one row per file placed by the core
//! (`nr_list_place`), drawing only visible rows. Rows alternate shading and the stripes
//! continue below the last file. The selection uses the accent bar whether or not the list is
//! focused, and names indent once per expanded parent folder. The core owns the rows, cells
//! and order (`nr_list_load`) and decides what presses, keys, drags, drops and renames do.

use super::chip;
use super::header::{self, Columns, Pressed};
use super::rename::{Ended, Rename, Renamed, Said};
use super::rows::{Key, Rows};
use super::scroll::Scroller;
use super::{
    DriveCommands, Item, drop_ring, file_icon, rows_of, selected, selection_bar, voiceover,
};
use crate::core::{c, contains, frame, offset, rect, text, zeroed};
use crate::draw::fill;
use crate::icons::{self, Glyph};
use crate::keys::held;
use crate::motion::now;
use crate::nr::NrDriveEdit;
use crate::text::{draw_line, width};
use crate::theme::{self, Face, Token, color};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable, Hitbox,
    HitboxBehavior, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollEvent, Styled, Window, canvas, div,
    point, prelude::*,
};
use northroom_core::ffi::{
    NrListRow, nr_folder_open_parent, nr_list_cell, nr_list_column_count, nr_list_count_label,
    nr_list_load, nr_list_open_in, nr_list_row, nr_list_row_layout, nr_list_toggle_expanded,
};
use northroom_core::ffi::{nr_list_drop, nr_shown_column};
use northroom_core::menus::ffi::nr_folder_drop;
use northroom_core::window::ffi::{
    NrDropPlace, nr_list_command_column, nr_list_drop_place, nr_list_place, nr_rows_context,
    nr_rows_selected,
};
use std::rc::Rc;

/// The list's table number in `nr_rows_*`.
pub const TABLE: i64 = -1;

/// One row as the core lists it (`NrListRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Item,
    pub cells: Vec<String>,
    pub depth: u32,
    pub expanded: bool,
    /// The parent folder's id; empty at the drive root.
    pub parent: String,
    /// A placeholder row's text ("Empty folder"); empty for a file row.
    pub placeholder: String,
}

/// What the list asks of the window.
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
    /// A right click on the header at `at`: Move items for column `moving` (-1 for none), then
    /// the list options (`menus::ffi::menu_list_options`).
    HeaderMenu { moving: i64, at: Point<Pixels> },
    /// The options button was pressed; open the options below it, right-aligned to it.
    Options { button: Bounds<Pixels> },
    /// The core's reply to a rename, drop or drive command, to be carried out.
    Said(Said),
}

pub struct List {
    focus: FocusHandle,
    rows: Vec<Row>,
    row_height: f32,
    frame: Bounds<Pixels>,
    table: Rows,
    pub scroller: Scroller,
    /// A header press the core is following (a column edge or the column itself).
    following: bool,
    /// The hovered row, if any. The list draws no hover; scripted checks read it.
    hover: Option<usize>,
    /// The folder row the dragged files would drop into.
    drop_target: Option<usize>,
    /// The row and files of the last drop query, while `drop_target` still holds its answer,
    /// so the same files over the same row are not asked again.
    drop_asked: Option<(i64, Vec<String>)>,
    rename: Option<Rename>,
}

impl EventEmitter<Event> for List {}

impl Focusable for List {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl List {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut list = Self {
            focus: cx.focus_handle(),
            rows: Vec::new(),
            row_height: nr_list_row_layout(0).height as f32,
            frame: Bounds::default(),
            table: Rows::new(TABLE),
            scroller: Scroller::default(),
            following: false,
            hover: None,
            drop_target: None,
            drop_asked: None,
            rename: None,
        };
        list.reload();
        list
    }

    /// Reloads the folder (`nr_list_load`), keeping the selection.
    pub fn reload(&mut self) {
        // The rows may be different files now, so re-ask any drag over them.
        self.drop_asked = None;
        let count = nr_list_load();
        let cells = nr_list_column_count();
        self.rows = (0..count)
            .filter_map(|index| {
                let mut row: NrListRow = zeroed();
                // SAFETY: a row to fill.
                if !unsafe { nr_list_row(index, &mut row) } {
                    return None;
                }
                Some(Row {
                    item: Item::of(&row.file),
                    cells: (1..cells)
                        .map(|column| text(&nr_list_cell(index, column)))
                        .collect(),
                    depth: row.depth,
                    expanded: row.expanded,
                    parent: text(&row.parent),
                    placeholder: text(&row.placeholder),
                })
            })
            .collect();
        self.row_height = nr_list_row_layout(0).height as f32;
    }

    /// Places the list at `frame`, the window's content rect (`nr_window_layout`).
    pub fn place(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hover
    }

    pub fn is_renaming(&self) -> bool {
        self.rename.is_some()
    }

    pub fn has_keys(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Focuses the list.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    /// The header and rows frames in the window.
    fn parts(&self) -> (Bounds<Pixels>, Bounds<Pixels>) {
        let place = nr_list_place(
            f64::from(self.frame.size.width),
            f64::from(self.frame.size.height),
        );
        let (x, y) = (
            f32::from(self.frame.origin.x),
            f32::from(self.frame.origin.y),
        );
        (
            offset(rect(&place.header), x, y),
            offset(rect(&place.rows), x, y),
        )
    }

    /// The row under `at`: `Some(-1)` for the space below the rows, `None` outside them.
    pub fn row_at(&self, at: Point<Pixels>) -> Option<i64> {
        let (_, rows) = self.parts();
        if !contains(&rows, at) || self.row_height <= 0.0 {
            return None;
        }
        let row =
            ((f32::from(at.y - rows.origin.y) + self.scroller.offset()) / self.row_height).floor();
        Some(if row >= 0.0 && (row as usize) < self.rows.len() {
            row as i64
        } else {
            -1
        })
    }

    /// Where the row named `title` is drawn in the window, for scripted checks.
    pub fn row_frame(&self, title: &str) -> Option<Bounds<Pixels>> {
        let row = self.rows.iter().position(|row| row.item.title == title)?;
        Some(self.frame_of(row))
    }

    fn frame_of(&self, row: usize) -> Bounds<Pixels> {
        let (_, rows) = self.parts();
        let top = f32::from(rows.origin.y) + row as f32 * self.row_height - self.scroller.offset();
        frame(
            f32::from(rows.origin.x),
            top,
            f32::from(rows.size.width),
            self.row_height,
        )
    }

    fn reveal(&mut self, row: usize) {
        let top = row as f32 * self.row_height;
        self.scroller.reveal(top, top + self.row_height);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.emit(Event::Changed);
        cx.notify();
    }

    /// Opens row `index`: a folder shows its files, a file fills the tab (`nr_list_open_in`).
    pub fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self
            .rows
            .get(index)
            .filter(|row| row.placeholder.is_empty())
        else {
            return;
        };
        if self.rename.is_some() {
            return;
        }
        let (parent, id) = (c(&row.parent), c(&row.item.id));
        // SAFETY: NUL-terminated ids.
        unsafe { nr_list_open_in(parent.as_ptr(), id.as_ptr()) };
        self.changed(cx);
    }

    // MARK: Rename

    /// Opens the rename field over `row`'s name.
    pub fn begin_rename(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shown) = self.rows.get(row).filter(|row| row.placeholder.is_empty()) else {
            return;
        };
        if self.rename.is_some() {
            return;
        }
        let (id, title) = (shown.item.id.clone(), shown.item.title.clone());
        self.reveal(row);
        let renamed = Renamed::File {
            column: nr_list_command_column(),
            id: id.clone(),
        };
        let rename = Rename::begin(
            &id,
            renamed,
            &title,
            window,
            cx,
            |list: &mut Self, ended, window, cx| list.end_rename(ended, window, cx),
        );
        self.rename = Some(rename);
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

    /// The rename field's rect over its row (the core's `rename` rect).
    fn rename_frame(&self, id: &str) -> Option<Bounds<Pixels>> {
        let row = self.rows.iter().position(|row| row.item.id == id)?;
        let bar = self.frame_of(row);
        let laid = Columns::lay_out(f32::from(bar.size.width));
        let name_x = f32::from(bar.origin.x) + laid.name_x;
        let x = name_x + nr_list_row_layout(self.rows[row].depth).rename as f32;
        Some(frame(
            x,
            f32::from(bar.origin.y) + 6.0,
            name_x + laid.name_width - x,
            20.0,
        ))
    }

    // MARK: Pointer

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let (header, rows) = self.parts();
        let at = event.position;
        if self.scroller.press(at, rows, now()) {
            cx.notify();
            return;
        }
        // A press on the list or its header focuses it.
        window.focus(&self.focus, cx);
        if contains(&header, at) {
            self.header_down(event, header, cx);
            return;
        }
        let Some(row) = self.row_at(at) else { return };
        if super::asks_for_menu(event) {
            self.table.forget();
            let about = rows_of(&nr_rows_context(TABLE, row));
            cx.emit(Event::RowMenu { row, about, at });
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        // A press in a folder's caret slot expands or collapses it.
        if self.toggle_folder_at(row, at, rows) {
            self.reload();
            self.changed(cx);
            return;
        }
        self.table
            .down(row, at, held(&event.modifiers), event.click_count);
        cx.notify();
    }

    /// Handles a press on the `header`: its menu, a sort, a width, or the column chooser.
    fn header_down(
        &mut self,
        event: &MouseDownEvent,
        header: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let at = event.position;
        let local = point(at.x - header.origin.x, at.y - header.origin.y);
        let header_width = f32::from(header.size.width);
        if super::asks_for_menu(event) {
            let moving = header::menu_column(header_width, f32::from(local.x));
            cx.emit(Event::HeaderMenu { moving, at });
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        match header::press(header_width, local, event.click_count) {
            Pressed::Nothing => {}
            Pressed::Changed => {
                self.reload();
                self.changed(cx);
            }
            Pressed::Options => {
                let laid = Columns::lay_out(header_width);
                cx.emit(Event::Options {
                    button: offset(
                        laid.chooser,
                        f32::from(header.origin.x),
                        f32::from(header.origin.y),
                    ),
                });
            }
            Pressed::Follow => self.following = true,
        }
        cx.notify();
    }

    /// Expands or collapses the folder at `row` when `at` is in its caret slot. True if it did.
    fn toggle_folder_at(&self, row: i64, at: Point<Pixels>, rows: Bounds<Pixels>) -> bool {
        let Some(shown) = usize::try_from(row).ok().and_then(|row| self.rows.get(row)) else {
            return false;
        };
        if !shown.item.holds_files || !shown.placeholder.is_empty() {
            return false;
        }
        let laid = Columns::lay_out(f32::from(rows.size.width));
        let caret =
            f32::from(rows.origin.x) + laid.name_x + nr_list_row_layout(shown.depth).caret as f32;
        let x = f32::from(at.x);
        let in_caret = x >= caret && x < caret + 16.0;
        if in_caret {
            let id = c(&shown.item.id);
            // SAFETY: a NUL-terminated id.
            unsafe { nr_list_toggle_expanded(id.as_ptr()) };
        }
        in_caret
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (header, rows) = self.parts();
        let at = event.position;
        let now = now();
        if self.following {
            if header::drag(f32::from(at.x - header.origin.x)) {
                cx.notify();
            }
            return;
        }
        if self.scroller.is_held() {
            self.scroller.pointer(at, rows, now);
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
            cx.notify();
            self.carry(&carried, at, window);
            return;
        }
        let hover = over
            .then(|| self.row_at(at))
            .flatten()
            .and_then(|row| usize::try_from(row).ok());
        if hover != self.hover {
            self.hover = hover;
        }
        if self.scroller.pointer(at, rows, now) {
            cx.notify();
        }
    }

    /// Starts dragging `carried` from `at` (empty for a drag that carries nothing).
    fn carry(&mut self, carried: &[usize], at: Point<Pixels>, window: &mut Window) {
        if carried.is_empty() {
            return;
        }
        let picked: Vec<&Row> = carried
            .iter()
            .filter_map(|&row| self.rows.get(row))
            .collect();
        let ids = picked.iter().map(|row| row.item.id.clone()).collect();
        let titles: Vec<String> = picked.iter().map(|row| row.item.title.clone()).collect();
        chip::begin(TABLE, ids, &titles, at);
        self.drag_over(self.row_at(at));
        window.refresh();
    }

    /// Rings only the folder a drag over `row` would drop into (-1 for the space below the
    /// rows, `None` when not over the list).
    ///
    /// Each query makes the core reload the list, so the same files over the same row reuse the
    /// last answer. Terminal rows are always re-asked, since whether they take files depends on
    /// what the session is doing.
    fn drag_over(&mut self, row: Option<i64>) {
        let Some((row, ids)) = row.zip(chip::carried()) else {
            self.drop_target = None;
            self.drop_asked = None;
            return;
        };
        let terminal = usize::try_from(row)
            .ok()
            .and_then(|row| self.rows.get(row))
            .is_some_and(|shown| shown.item.kind == northroom_core::ffi::NrFileKind::Terminal);
        let asked = self
            .drop_asked
            .as_ref()
            .is_some_and(|(asked, carried)| *asked == row && *carried == ids);
        if asked && !terminal {
            return;
        }
        let place = self.drop_place(&ids, row);
        self.drop_target =
            matches!(place, NrDropPlace::Folder | NrDropPlace::Terminal).then_some(row as usize);
        self.drop_asked = Some((row, ids));
    }

    fn drop_place(&self, ids: &[String], row: i64) -> NrDropPlace {
        let ids = c(&ids.join("\n"));
        // SAFETY: a NUL-terminated list.
        unsafe { nr_list_drop_place(ids.as_ptr(), row) }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, over: bool, cx: &mut Context<Self>) {
        let (header, _) = self.parts();
        if self.following {
            self.following = false;
            if header::release(f32::from(event.position.x - header.origin.x)) {
                self.reload();
                self.changed(cx);
            }
            return;
        }
        if self.scroller.release(now()) {
            cx.notify();
            return;
        }
        if let Some(drag) = chip::current() {
            self.drop_target = None;
            self.drop_asked = None;
            if let Some(row) = over.then(|| self.row_at(event.position)).flatten() {
                chip::end();
                self.drop(&drag.ids, row, cx);
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

    /// Drops `ids` onto `row`, where the core decides.
    fn drop(&mut self, ids: &[String], row: i64, cx: &mut Context<Self>) {
        let joined = c(&ids.join("\n"));
        let target = || {
            c(&self
                .rows
                .get(row as usize)
                .map(|row| row.item.id.clone())
                .unwrap_or_default())
        };
        let said = match self.drop_place(ids, row) {
            NrDropPlace::Nowhere => return,
            NrDropPlace::Folder => {
                let folder = target();
                // SAFETY: NUL-terminated strings.
                Said::refused(&unsafe { nr_list_drop(joined.as_ptr(), folder.as_ptr()) })
            }
            NrDropPlace::Shown => {
                let shown = c("");
                // SAFETY: NUL-terminated strings.
                Said::refused(&unsafe { nr_list_drop(joined.as_ptr(), shown.as_ptr()) })
            }
            NrDropPlace::Terminal => {
                let terminal = target();
                // SAFETY: NUL-terminated strings.
                Said::of(&unsafe {
                    nr_folder_drop(nr_list_command_column(), joined.as_ptr(), terminal.as_ptr())
                })
            }
        };
        self.reload();
        cx.emit(Event::Said(said));
        self.changed(cx);
    }

    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if self.scroller.wheel(event, now()) {
            cx.notify();
        }
    }

    /// The scroll offset, visible height and content height when `at` is over the rows
    /// (`ev:scrolled`).
    pub fn scrolled_at(&self, at: Point<Pixels>) -> Option<(f32, f32, f32)> {
        let (_, rows) = self.parts();
        contains(&rows, at).then(|| {
            (
                self.scroller.offset(),
                f32::from(rows.size.height),
                self.rows.len() as f32 * self.row_height,
            )
        })
    }

    // MARK: Keys

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        // The rename field takes keys while renaming.
        if !self.focus.is_focused(window) {
            return;
        }
        if event.keystroke.key == "escape" && chip::end().is_some() {
            // Escape cancels a drag in progress without moving anything.
            self.drop_target = None;
            self.drop_asked = None;
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
            Key::Reload => {
                self.reload();
                self.changed(cx);
            }
            Key::Open(row) => self.open(row, cx),
            Key::OpenParent => {
                nr_folder_open_parent();
                self.changed(cx);
            }
            Key::Rename(row) => self.begin_rename(row, window, cx),
            // Trash the selection without asking; ⌘Z undoes it.
            Key::Trash => {
                let said = super::edit_files(NrDriveEdit::Trash, TABLE);
                self.reload();
                cx.emit(Event::Said(said));
                self.changed(cx);
            }
            Key::Column(_) => {}
        }
        cx.stop_propagation();
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let (header, rows_frame) = self.parts();
        let now = now();
        self.scroller.set_extent(
            f32::from(rows_frame.size.height),
            self.rows.len() as f32 * self.row_height,
        );
        let laid = Columns::lay_out(f32::from(header.size.width));
        window.with_content_mask(Some(gpui::ContentMask { bounds: rows_frame }), |window| {
            self.paint_rows(window, cx, rows_frame, &laid);
            self.scroller.paint_bar(window, rows_frame, now);
        });
        let count = text(&nr_list_count_label(nr_rows_selected(TABLE).count));
        header::paint(window, cx, header, &laid, &count);
        if self.scroller.tick(now) {
            window.request_animation_frame();
        }
        if self.following
            || contains(&header, window.mouse_position())
                && laid.on_edge(f32::from(window.mouse_position().x - header.origin.x))
        {
            window.set_cursor_style(gpui::CursorStyle::ResizeLeftRight, hitbox);
        }
        self.listen(hitbox, window, cx);
    }

    /// Listens for pointer events over the list, and anywhere in the window while a followed
    /// press is held.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        Self::listen_for_buttons(&entity, hitbox, window);
        let (list, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = over.is_hovered(window);
                list.update(cx, |list, cx| list.mouse_move(event, hovered, window, cx));
            }
        });
        let (list, over) = (entity, hitbox.clone());
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                list.update(cx, |list, cx| list.scroll(event, cx));
            }
        });
    }

    /// Routes to the list a press over its `hitbox` and any left-button release.
    fn listen_for_buttons(entity: &Entity<Self>, hitbox: &Hitbox, window: &mut Window) {
        let (list, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                list.update(cx, |list, cx| list.mouse_down(event, window, cx));
            }
        });
        let (list, over) = (entity.clone(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                let hovered = over.is_hovered(window);
                list.update(cx, |list, cx| list.mouse_up(event, hovered, cx));
            }
        });
    }

    fn paint_rows(
        &self,
        window: &mut Window,
        cx: &mut App,
        rows_frame: Bounds<Pixels>,
        laid: &Columns,
    ) {
        let (x0, y0) = (
            f32::from(rows_frame.origin.x),
            f32::from(rows_frame.origin.y),
        );
        let row_width = f32::from(rows_frame.size.width);
        let height = self.row_height;
        let scrolled = self.scroller.offset();
        let selected = selected(TABLE);
        // Visible rows only. Stripes continue below the last file, never above the first.
        let first = (scrolled / height).floor().max(0.0) as usize;
        let last = ((scrolled + f32::from(rows_frame.size.height)) / height)
            .ceil()
            .max(0.0) as usize;
        for index in first..=last {
            let bounds = frame(x0, y0 + index as f32 * height - scrolled, row_width, height);
            if index % 2 == 1 {
                fill(
                    window,
                    bounds,
                    theme::laid_over(Token::Foreground, 0.03, Token::Background),
                );
            }
            let Some(row) = self.rows.get(index) else {
                continue;
            };
            let is_selected = selected.binary_search(&index).is_ok();
            if is_selected {
                let above = index > 0 && selected.binary_search(&(index - 1)).is_ok();
                let below = selected.binary_search(&(index + 1)).is_ok();
                // The selection uses the accent whether or not the list is focused.
                selection_bar(window, bounds, true, above, below);
            }
            if self.drop_target == Some(index) {
                drop_ring(window, bounds, is_selected);
            }
            let renaming = self
                .rename
                .as_ref()
                .is_some_and(|rename| rename.id == row.item.id);
            paint_row(window, cx, row, bounds, laid, is_selected, renaming);
        }
    }
}

/// Draws one row in one pass. With `renaming`, the field over it draws the name.
fn paint_row(
    window: &mut Window,
    cx: &mut App,
    row: &Row,
    bounds: Bounds<Pixels>,
    laid: &Columns,
    emphasized: bool,
    renaming: bool,
) {
    let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
    let text_color = if emphasized {
        color(Token::AccentForeground)
    } else {
        color(Token::Foreground)
    };
    let font = Face::sans(12.0, 400.0);
    // Indent once per expanded parent folder.
    let layout = nr_list_row_layout(row.depth);
    let name_x = x0 + laid.name_x;
    let name_end = name_x + laid.name_width;
    if !row.placeholder.is_empty() {
        let x = name_x + layout.placeholder as f32;
        draw_line(
            window,
            cx,
            &row.placeholder,
            font,
            color(Token::MutedForeground),
            x,
            y0 + 8.0,
            16.0,
            name_end - x,
            None,
        );
        return;
    }
    // A 16 px slot for a folder's disclosure caret, a 24 px slot for the icon, then the name.
    if row.item.holds_files {
        let glyph = if row.expanded {
            Glyph::CaretDown
        } else {
            Glyph::CaretRight
        };
        icons::draw(
            window,
            cx,
            glyph,
            frame(name_x + layout.caret as f32 + 2.0, y0 + 10.0, 12.0, 12.0),
            text_color,
        );
    }
    file_icon(
        window,
        cx,
        &row.item,
        frame(name_x + layout.icon as f32, y0 + 4.0, 24.0, 24.0),
        text_color,
    );
    let x = name_x + layout.name as f32;
    if !renaming {
        paint_title(window, cx, row, x, y0, name_end, text_color);
    }
    // Other cells use the same text colour at 70%.
    paint_other_cells(window, cx, row, laid, x0, y0, theme::faded(text_color, 0.7));
}

/// Draws the row's name from `x` to `name_end`.
fn paint_title(
    window: &mut Window,
    cx: &mut App,
    row: &Row,
    x: f32,
    y0: f32,
    name_end: f32,
    ink: Hsla,
) {
    draw_line(
        window,
        cx,
        &row.item.title,
        Face::sans(12.0, 400.0),
        ink,
        x,
        y0 + 8.0,
        16.0,
        name_end - x,
        None,
    );
}

/// Draws the row's other cells in their columns, in `ink`.
fn paint_other_cells(
    window: &mut Window,
    cx: &mut App,
    row: &Row,
    laid: &Columns,
    x0: f32,
    y0: f32,
    ink: Hsla,
) {
    let font = Face::sans(12.0, 400.0);
    for (index, column) in laid.others.iter().enumerate() {
        let Some(words) = row.cells.get(index) else {
            break;
        };
        // Right-aligned cells are numbers, so use tabular numerals.
        let face = if column.right_aligned {
            font.tabular()
        } else {
            font
        };
        let start = x0 + column.x;
        let at = if column.right_aligned {
            start.max(start + column.width - width(words, face))
        } else {
            start
        };
        draw_line(
            window,
            cx,
            words,
            face,
            ink,
            at,
            y0 + 8.0,
            16.0,
            column.width,
            None,
        );
    }
}

impl List {
    /// Every row's accessibility node, built only while assistive tech reads the window.
    fn heard(&self, window: &Window) -> Vec<voiceover::HeardRow> {
        if !window.is_a11y_active() {
            return Vec::new();
        }
        let chosen = selected(TABLE);
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| voiceover::HeardRow {
                name: if row.placeholder.is_empty() {
                    row.item.title.clone()
                } else {
                    row.placeholder.clone()
                },
                selected: chosen.contains(&index),
                frame: self.frame_of(index),
            })
            .collect()
    }
}

impl Render for List {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(rename) = &self.rename
            && let Some(at) = self.rename_frame(&rename.id)
        {
            rename.place(at, cx);
        }
        let list = cx.entity();
        let commanded = list.clone();
        let frame = self.frame;
        let field = self.rename.as_ref().map(|rename| rename.field.clone());
        let heard = self.heard(window);
        div()
            .id(("kit-list", cx.entity_id()))
            .absolute()
            .left(frame.origin.x)
            .top(frame.origin.y)
            .w(frame.size.width)
            .h(frame.size.height)
            .role(voiceover::SCROLL_AREA)
            .a11y_synthetic_children(voiceover::scroll_area())
            // Copy files dropped from outside into the shown folder, wherever they land.
            .on_drop(cx.listener(|list, dropped: &gpui::ExternalPaths, _, cx| {
                let said = super::import(nr_shown_column(), dropped);
                list.reload();
                cx.emit(Event::Said(said));
                list.changed(cx);
            }))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        list.update(cx, |list, cx| list.paint(&hitbox, window, cx))
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
                    .aria_row_count(self.rows.len())
                    .track_focus(&self.focus)
                    .key_context(super::CONTEXT)
                    .take_drive_commands(
                        &self.focus,
                        || TABLE,
                        Rc::new(move |said, _, cx| {
                            commanded.update(cx, |list, cx| {
                                list.reload();
                                cx.emit(Event::Said(said));
                                list.changed(cx);
                            })
                        }),
                    )
                    .on_key_down(cx.listener(Self::key_down))
                    .children(voiceover::rows(heard, frame.origin))
                    .children(voiceover::column("row"))
                    // The field uses window points, so its parent sits at the window origin.
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
