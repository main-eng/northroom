//! The folder as columns in a horizontal scroller. Each open folder is a `kit::lists::column`,
//! laid out side by side by the core (`NrWindowLayout::columns_scroll`, `column_width`), with the
//! open file beside the last one.
//!
//! The columns scroll horizontally together on `kit::lists::scroll`, which takes horizontal
//! gestures over them; each column keeps vertical ones. Over the file, a gesture slides the
//! columns only until the file is fully in view (`reveals_file`), then belongs to the file. A
//! newly opened column or newly shown file scrolls into view (`reveal_start`, `reveal_end`), as
//! does the column ← or → moves to.
//!
//! The core decides what presses, keys, drags and renames do. This screen creates and places
//! the columns, gives the keys to the column ← or → moves to, and turns column requests into
//! menus and dialogs. It registers the check steps for columns and their scroll (`register`).

use super::{Ask, Folder, Open, Screen, folder_menu, on_bubble};
use crate::script::probe::Probe;
use crate::script::{Answer, Pointer, Route as Way, Step, Table, workspace};
use gpui::{
    App, AppContext, Bounds, Context, DispatchPhase, Entity, EventEmitter, FocusHandle, Focusable,
    HitboxBehavior, IntoElement, ParentElement, Pixels, Point, Render, ScrollEvent, Styled,
    Subscription, Window, canvas, div,
};
use kit::core::{contains, frame, rect, text};
use kit::lists::column::{self, Column};
use kit::lists::scroll::{Route, Scroller};
use kit::lists::selected;
use kit::motion::now;
use kit::nr::*;
use std::rc::Rc;

pub struct Columns {
    pub columns: Vec<Entity<Column>>,
    /// The scroller's window frame, a column's width, and the total width of the columns and
    /// the file.
    frame: Bounds<Pixels>,
    width: f32,
    content: f32,
    /// Hidden when the folder is a list or board, or an opened file fills the tab.
    hidden: bool,
    /// The horizontal scroll offset, and which gestures the row owns.
    scroller: Scroller,
    route: Route,
    /// The current gesture began over the columns rather than the file, so it stays theirs when
    /// it crosses onto the file.
    owned_from_columns: bool,
    /// A column was just opened or a file just shown; scroll it into view after the next
    /// layout.
    revealing: bool,
    /// Identifies the shown file, to detect a newly shown one (`nr_opened_identity`).
    opened_key: String,
    events: Vec<Subscription>,
}

impl Columns {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            columns: Vec::new(),
            frame: Bounds::default(),
            width: 0.0,
            content: 0.0,
            hidden: true,
            scroller: Scroller::default(),
            route: Route::new(true),
            owned_from_columns: false,
            revealing: false,
            opened_key: String::new(),
            events: Vec::new(),
        }
    }

    /// The horizontal scroll offset as drawn, for the views beside the columns.
    pub fn scroll(&self) -> f32 {
        self.scroller.offset()
    }

    /// Column `index`'s window frame.
    fn frame_of(&self, index: usize) -> Bounds<Pixels> {
        frame(
            f32::from(self.frame.origin.x) + index as f32 * self.width - self.scroll(),
            f32::from(self.frame.origin.y),
            self.width,
            f32::from(self.frame.size.height),
        )
    }

    /// Gives column `index` the keys and scrolls it into view.
    fn focus(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        if let Some(column) = self.columns.get(index).cloned() {
            column.update(cx, |column, cx| column.focus(window, cx));
            let start = index as f32 * self.width;
            self.scroller.reveal(start, start + self.width);
            self.place_columns(cx);
        }
    }

    /// Whether `at` is over a column rather than the file beside them. The file scrolls itself
    /// (a table, a PDF, a live page), so gestures over it never move the columns.
    fn columns_hold(&self, at: Point<Pixels>) -> bool {
        let end =
            f32::from(self.frame.origin.x) + self.columns.len() as f32 * self.width - self.scroll();
        contains(&self.frame, at) && f32::from(at.x) < end
    }

    /// Whether a wheel or trackpad `event` over the file slides the columns first. It does when
    /// it moves horizontally toward the file and the columns can still scroll to show more of
    /// it. Once the file is fully in view, or the gesture reverses, it belongs to the file; the
    /// columns scroll back only from over the columns.
    fn reveals_file(&self, event: &ScrollEvent) -> bool {
        if self.hidden
            || !contains(&self.frame, event.position)
            || self.columns_hold(event.position)
        {
            return false;
        }
        // A swipe that began over the columns stays theirs as it crosses onto the file.
        if self.route.owns_gesture() && self.owned_from_columns {
            return true;
        }
        let (across, down) = match event.delta {
            gpui::ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            gpui::ScrollDelta::Lines(delta) => (delta.x, delta.y),
        };
        let furthest = (self.content - f32::from(self.frame.size.width)).max(0.0);
        // Leftward travel reveals what is further right, the file.
        across < 0.0 && across.abs() > down.abs() && self.scroll() < furthest - 0.5
    }

    /// Places each column at the row's current scroll.
    fn place_columns(&self, cx: &mut App) {
        for (index, column) in self.columns.iter().enumerate() {
            let at = self.frame_of(index);
            column.update(cx, |column, cx| column.place(at, cx));
        }
    }

    /// Paints the horizontal scrollbar, frame by frame while it fades.
    fn paint_scrollbar(&mut self, window: &mut Window) {
        let now = now();
        let view = self.frame;
        self.scroller.paint_bar_across(window, view, now);
        if self.scroller.tick(now) {
            window.request_animation_frame();
        }
    }

    fn scroll_event(&mut self, event: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        let mut moved = false;
        let owned_before = self.route.owns_gesture();
        let owned = self.route.own(event);
        if !owned_before && self.route.owns_gesture() {
            self.owned_from_columns = self.columns_hold(event.position);
        }
        for own in owned {
            // The scroller has one axis, so pass horizontal travel as if it were vertical.
            let mut across = own.clone();
            across.delta = match own.delta {
                gpui::ScrollDelta::Pixels(delta) => {
                    gpui::ScrollDelta::Pixels(gpui::point(delta.y, delta.x))
                }
                gpui::ScrollDelta::Lines(delta) => {
                    gpui::ScrollDelta::Lines(gpui::point(delta.y, delta.x))
                }
            };
            moved |= self.scroller.wheel(&across, now);
        }
        if moved {
            self.place_columns(cx);
            // What sits beside the columns moves with them.
            window.refresh();
        }
    }

    /// Gives column `index` of `columns` the keys.
    fn focus_in(columns: &Entity<Self>, index: usize, window: &mut Window, cx: &mut App) {
        if let Some(column) = columns.read(cx).columns.get(index).cloned() {
            column.update(cx, |column, cx| column.focus(window, cx));
        }
    }

    /// Puts the rename field over file `id`'s row in the last column showing it; false if none
    /// does.
    pub fn rename(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let found = self.columns.iter().rev().find_map(|column| {
            let row = column
                .read(cx)
                .items()
                .iter()
                .position(|item| item.id == id)?;
            Some((column.clone(), row))
        });
        let Some((column, row)) = found else {
            return false;
        };
        column.update(cx, |column, cx| column.begin_rename(row, window, cx));
        true
    }

    fn column_event(
        &mut self,
        column: &Entity<Column>,
        event: &column::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            column::Event::Changed => cx.emit(Ask::Changed),
            column::Event::Column(index) => {
                // ← or → moved the selection in the core; that column takes the keys.
                cx.emit(Ask::Changed);
                self.focus(*index, window, cx);
                window.refresh();
            }
            column::Event::RowMenu { row, about, at } => {
                let column = column.read(cx);
                let items = column.items();
                let ids: Vec<String> = about
                    .iter()
                    .filter_map(|&row| items.get(row))
                    .map(|item| item.id.clone())
                    .collect();
                let clicked = usize::try_from(*row)
                    .ok()
                    .and_then(|row| items.get(row))
                    .map(|item| item.id.clone())
                    .unwrap_or_default();
                let which = if clicked.is_empty() {
                    NrMenu::Background
                } else {
                    NrMenu::File
                };
                let again = folder_menu(which, column.index, &clicked, &ids);
                let index = column.index;
                // A row's Open selects it, opening a folder in the next column or showing the
                // file beside. Selected terminals open side by side, filling the tab.
                let open = usize::try_from(*row)
                    .ok()
                    .filter(|_| !clicked.is_empty())
                    .map(|row| -> Open {
                        Rc::new(move |_, _| nr_column_open_from_menu(index, row))
                    });
                cx.emit(Ask::FileMenu {
                    which,
                    again,
                    at: *at,
                    folder: Folder {
                        column: index,
                        open,
                    },
                });
            }
            column::Event::Said(said) => cx.emit(Ask::Said(said.clone())),
        }
    }

    /// Each column's selection and whether it has the keys, for a check.
    fn said(&self, window: &Window, cx: &App) -> String {
        self.columns
            .iter()
            .map(|column| {
                let column = column.read(cx);
                let rows: Vec<String> = selected(column.index as i64)
                    .iter()
                    .map(usize::to_string)
                    .collect();
                format!(
                    "column {} rows {} focused {}",
                    column.index,
                    rows.join(","),
                    column.focus_handle(cx).is_focused(window)
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    }
}

impl EventEmitter<Ask> for Columns {}

impl Screen for Columns {
    /// Reads each column the core has open again (`nr_shell_view`).
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = nr_shell_view().columns;
        // After Back or a tab switch, no removed column may keep the keys.
        if self.columns[count.min(self.columns.len())..]
            .iter()
            .any(|column| column.focus_handle(cx).contains_focused(window, cx))
        {
            window.blur(cx);
        }
        self.columns.truncate(count);
        self.events.truncate(count);
        // Scroll a newly opened column or newly shown file into view.
        let key = text(&nr_opened_identity());
        self.revealing |= self.columns.len() < count || (key != self.opened_key && !key.is_empty());
        self.opened_key = key;
        while self.columns.len() < count {
            let index = self.columns.len();
            let column = cx.new(|cx| Column::new(index, cx));
            self.events
                .push(cx.subscribe_in(&column, window, Self::column_event));
            self.columns.push(column);
        }
        for (index, column) in self.columns.iter().enumerate() {
            let at = self.frame_of(index);
            column.update(cx, |column, cx| {
                column.reload();
                column.place(at, cx);
                cx.notify();
            });
        }
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, laid: &NrWindowLayout, cx: &mut Context<Self>) {
        self.frame = frame;
        self.width = laid.column_width as f32;
        self.hidden = laid.columns_hidden;
        self.content = f32::from(rect(&laid.columns_content).size.width);
        self.scroller
            .set_extent(f32::from(frame.size.width), self.content);
        if std::mem::take(&mut self.revealing) {
            // The file is last in the row and is what to reveal.
            if laid.reveals {
                self.scroller
                    .reveal(laid.reveal_start as f32, laid.reveal_end as f32);
            }
        }
        self.place_columns(cx);
    }

    fn keys(&self, cx: &App) -> Option<FocusHandle> {
        self.columns.last().map(|column| column.focus_handle(cx))
    }

    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App) {
        let titles = |column: &Column| -> Vec<String> {
            let items = column.items();
            selected(column.index as i64)
                .into_iter()
                .filter_map(|row| items.get(row).map(|item| item.title.clone()))
                .collect()
        };
        probe.columns = self.columns.len();
        if probe.folder != NrFolderView::Columns {
            return;
        }
        probe.rows = self.columns.last().map_or_else(Vec::new, |column| {
            column
                .read(cx)
                .items()
                .iter()
                .map(|item| item.title.clone())
                .collect()
        });
        let selections: Vec<String> = self
            .columns
            .iter()
            .map(|column| titles(column.read(cx)).join(","))
            .collect();
        probe.said = selections.join(" / ");
        probe.selected = selections;
        probe.renaming = self
            .columns
            .iter()
            .any(|column| column.read(cx).is_renaming());
        if self
            .columns
            .iter()
            .any(|column| column.focus_handle(cx).is_focused(window))
        {
            probe.keys = "RowPointerTableView".to_owned();
        }
    }

    /// The column under `at`, when the columns are drawn.
    fn scrolled_at(&self, at: Point<Pixels>, cx: &App) -> Option<(f32, f32, f32)> {
        if self.hidden || !contains(&self.frame, at) {
            return None;
        }
        self.columns
            .iter()
            .find_map(|column| column.read(cx).scrolled_at(at))
    }

    /// The row named `title` in the last column holding it.
    fn point_of(&self, title: &str, cx: &App) -> Option<Point<Pixels>> {
        self.columns
            .iter()
            .rev()
            .find_map(|column| column.read(cx).row_frame(title))
            .map(|frame| frame.center())
    }
}

impl Render for Columns {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame = self.frame;
        let columns = cx.entity();
        // Columns are placed in window points and clipped at the scroller's edges.
        div()
            .absolute()
            .left(frame.origin.x)
            .top(frame.origin.y)
            .w(frame.size.width)
            .h(frame.size.height)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left(-frame.origin.x)
                    .top(-frame.origin.y)
                    .children(self.columns.clone()),
            )
            .child(
                // The horizontal scrollbar, and wheel handling over any column.
                canvas(
                    // Hovered unless something on top (a menu, a toast) takes the pointer.
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, over, window, cx| {
                        let entity = columns.clone();
                        columns.update(cx, |columns, _| columns.paint_scrollbar(window));
                        let first = entity.clone();
                        // Runs before the file sees it, so a gesture that reveals more of the
                        // file slides the columns first (`reveals_file`).
                        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && over.is_hovered(window)
                                && first.read(cx).reveals_file(event)
                            {
                                first.update(cx, |columns, cx| {
                                    columns.scroll_event(event, window, cx)
                                });
                                cx.stop_propagation();
                            }
                        });
                        on_bubble(window, move |event: &ScrollEvent, window, cx| {
                            entity.update(cx, |columns, cx| {
                                if !columns.hidden && columns.columns_hold(event.position) {
                                    columns.scroll_event(event, window, cx);
                                }
                            });
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Way::Kind(NrScriptKind::Columns), step);
    table.answer(Way::Event(NrScriptEvent::LookColumns), look);
}

/// The horizontal scroll offset, scroller width and total width with the file, reported by
/// the core once scrolling settles.
fn look(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    window.draw(cx).clear(cx);
    let columns = workspace(window).read(cx).columns.read(cx);
    nr_script_looked(
        f64::from(columns.scroller.offset()),
        f64::from(columns.frame.size.width),
        f64::from(columns.content),
        true,
    );
    Answer::Done
}

/// A columns step: focus a column, select rows `x` to `y`, or press a key in it. Then reports
/// each column's selection.
fn step(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let workspace = workspace(window);
    let columns = workspace.read(cx).columns.clone();
    let index = step.number;
    let present = usize::try_from(index)
        .ok()
        .filter(|&index| index < columns.read(cx).columns.len());
    if let Some(column) = present {
        match step.columns {
            NrColumnsScript::Focus => Columns::focus_in(&columns, column, window, cx),
            NrColumnsScript::Group if step.x >= 0.0 => {
                let rows: Vec<usize> = (step.x as usize..=step.y as usize).collect();
                // SAFETY: `rows` holds `len` rows.
                unsafe { nr_rows_set(column as i64, rows.as_ptr(), rows.len()) };
                Columns::focus_in(&columns, column, window, cx);
            }
            NrColumnsScript::Key => {
                Columns::focus_in(&columns, column, window, cx);
                let name = kit::keys::gpui_name(step.key).unwrap_or_default();
                window.draw(cx).clear(cx);
                window.dispatch_keystroke(
                    gpui::Keystroke {
                        modifiers: step.held(),
                        key: name,
                        key_char: None,
                    },
                    cx,
                );
            }
            _ => {}
        }
    }
    workspace.update(cx, |workspace, cx| workspace.changed(window, cx));
    window.draw(cx).clear(cx);
    let said = columns.read(cx).said(window, cx);
    eprintln!("columns\t{}\t{said}", step.name);
    Answer::Done
}
