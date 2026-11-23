//! The folder as a list, drawn by `kit::lists::list`. Kit owns the header, rows, scroller, rename
//! field and drags; the core decides what presses, keys, drags and renames do. This screen places
//! the list in the window and turns its requests into menus, dialogs and effects.

use super::{Ask, Folder, Open, Screen, folder_menu};
use crate::script::probe::Probe;
use gpui::{
    App, AppContext, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Pixels, Point, Render, Subscription, Window, point, px,
};
use kit::core::c;
use kit::lists::list::{self, List};
use kit::lists::selected;
use kit::menu::Again;
use kit::nr::*;
use std::rc::Rc;

pub struct ListScreen {
    pub list: Entity<List>,
    _events: Subscription,
}

impl ListScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = cx.new(List::new);
        let events = cx.subscribe_in(&list, window, Self::list_event);
        Self {
            list,
            _events: events,
        }
    }

    /// Puts the rename field over the row of file `id`; false when no row shows it.
    pub fn rename(&self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let row = self
            .list
            .read(cx)
            .rows()
            .iter()
            .position(|row| row.item.id == id);
        let Some(row) = row else { return false };
        self.list
            .update(cx, |list, cx| list.begin_rename(row, window, cx));
        true
    }

    fn list_event(
        &mut self,
        list: &Entity<List>,
        event: &list::Event,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            list::Event::Changed => cx.emit(Ask::Changed),
            list::Event::RowMenu { row, about, at } => {
                let rows = list.read(cx).rows();
                let ids: Vec<String> = about
                    .iter()
                    .filter_map(|&row| rows.get(row))
                    .map(|row| row.item.id.clone())
                    .collect();
                // A placeholder row ("Empty folder") counts as empty space.
                let clicked = usize::try_from(*row)
                    .ok()
                    .and_then(|row| rows.get(row))
                    .filter(|row| row.placeholder.is_empty());
                let column = nr_list_command_column();
                let Some(clicked) = clicked else {
                    let which = NrMenu::Background;
                    cx.emit(Ask::FileMenu {
                        which,
                        again: folder_menu(which, column, "", &ids),
                        at: *at,
                        folder: Folder { column, open: None },
                    });
                    return;
                };
                let which = NrMenu::File;
                let again = folder_menu(which, column, &clicked.item.id, &ids);
                // Its Open opens the file in the folder the list shows it in.
                let (parent, id) = (clicked.parent.clone(), clicked.item.id.clone());
                let open: Open = Rc::new(move |_, _| {
                    let (parent, id) = (c(&parent), c(&id));
                    // SAFETY: NUL-terminated ids.
                    unsafe { nr_list_open_in(parent.as_ptr(), id.as_ptr()) };
                });
                cx.emit(Ask::FileMenu {
                    which,
                    again,
                    at: *at,
                    folder: Folder {
                        column,
                        open: Some(open),
                    },
                });
            }
            list::Event::HeaderMenu { moving, at } => {
                let moving = *moving;
                let again: Again = Rc::new(move |_| menu_list_options(u32::try_from(moving).ok()));
                cx.emit(Ask::Menu {
                    which: NrMenu::ListOptions,
                    again,
                    at: *at,
                    below: false,
                });
            }
            list::Event::Options { button } => {
                // The options menu hangs under its button, right edges aligned.
                let again: Again = Rc::new(|_| menu_list_options(None));
                let width = kit::menu::Menus::width_of(&again(cx));
                let at = point(
                    button.origin.x + button.size.width - px(width),
                    button.origin.y + button.size.height + px(4.0),
                );
                cx.emit(Ask::Menu {
                    which: NrMenu::ListOptions,
                    again,
                    at,
                    below: true,
                });
            }
            list::Event::Said(said) => cx.emit(Ask::Said(said.clone())),
        }
    }
}

impl EventEmitter<Ask> for ListScreen {}

impl Screen for ListScreen {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| {
            list.reload();
            cx.notify();
        });
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        self.list.update(cx, |list, cx| list.place(frame, cx));
    }

    fn keys(&self, cx: &App) -> Option<FocusHandle> {
        Some(self.list.focus_handle(cx))
    }

    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App) {
        let list = self.list.read(cx);
        let titles: Vec<String> = list
            .rows()
            .iter()
            .map(|row| row.item.title.clone())
            .collect();
        let chosen: Vec<String> = selected(list::TABLE)
            .into_iter()
            .filter_map(|row| titles.get(row).cloned())
            .collect();
        let expanded: Vec<String> = list
            .rows()
            .iter()
            .filter(|row| row.expanded)
            .map(|row| row.item.title.clone())
            .collect();
        probe.folder = NrFolderView::List;
        probe.said = format!(
            "list {} rows; selected {}; open {}{}",
            titles.len(),
            chosen.join(","),
            expanded.join(","),
            if list.is_renaming() { "; renaming" } else { "" }
        );
        probe.rows = titles;
        probe.selected = chosen;
        probe.expanded = expanded;
        probe.renaming = list.is_renaming();
        if list.has_keys(window) {
            probe.keys = "RowPointerTableView".to_owned();
        }
    }

    fn scrolled_at(&self, at: Point<Pixels>, cx: &App) -> Option<(f32, f32, f32)> {
        self.list.read(cx).scrolled_at(at)
    }

    fn point_of(&self, title: &str, cx: &App) -> Option<Point<Pixels>> {
        self.list
            .read(cx)
            .row_frame(title)
            .map(|frame| frame.center())
    }
}

impl Render for ListScreen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.list.clone()
    }
}
