//! Drawn menus. The core describes the menu (`menus::Menu`), lays out its rows
//! (`nr_menu_layout`), handles keys and hover (`nr_menu_key`, `nr_menu_hover`) and places it
//! (`nr_menu_place`). This module draws it on the popover surface with a 1 px border and 10 px
//! corners, including submenus, the field, ticks, shortcuts and the core's entrance and exit.
//! While a lit row names a terminal (`Entry::peek`), its last lines show on a panel beside the
//! menu, read and placed by the core (`sessions::peek`).
//!
//! As in GPUI Kit's popover and dialog, open menus share one deferred layer at the window
//! origin. Each panel's hitbox blocks the pointer from what is under it, as AppKit's menu window
//! does. A press anywhere else closes every menu and doesn't reach what is under it. The layer
//! takes key focus while a menu is open and returns it on close.

use crate::core::{c, contains, rect};
use crate::icons::{self, Glyph};
use crate::menu_model::{Check, Entry, Kind, Menu};
use crate::motion::{Move, Pose, number};
use crate::popover::{Layer, place};
use crate::theme::{Face, Token, color, mixed};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, FocusHandle, HitboxBehavior, Hsla, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Render,
    ScrollDelta, ScrollEvent, Task, Window, canvas, div, point, prelude::*, px, size,
};
use northroom_core::ffi::{
    NR_WHEEL_CLICKS, NR_WHEEL_POINTS, NrKeyPress, NrMotion, nr_appearance_is_dark,
    nr_wheel_scrolled,
};
use northroom_core::menus::ffi::{
    NrMenuIntent, NrMenuItem, NrMenuRow, NrMenuRowKind, NrMenuStyle, nr_menu_first_row,
    nr_menu_hover, nr_menu_key, nr_menu_layout, nr_menu_light, nr_menu_replacing_y,
    nr_menu_stays_lit,
};
use northroom_core::sessions::peek;
use std::ffi::CString;
use std::rc::Rc;
use std::time::Duration;

/// Runs the chosen entry.
pub type Run = Rc<dyn Fn(&Entry, &mut Window, &mut App)>;
/// Rebuilds the menu after an entry that keeps it open.
pub type Again = Rc<dyn Fn(&mut App) -> Menu>;

/// How far a context menu opens from the pointer.
const CONTEXT_GAP: f64 = 2.0;

/// How often a shown peek reads its terminal's screen again.
const PEEK_EVERY: Duration = Duration::from_millis(400);

/// A terminal's last lines, shown beside the menu while the row that names it is lit
/// (`Entry::peek`).
struct Peek {
    /// The terminal file.
    file: String,
    lines: Vec<String>,
    /// Reads the lines again as the screen changes, for as long as the peek is kept.
    _follow: Task<()>,
}

/// Where one row's parts go, copied from the core's `NrMenuRowPlace`.
#[derive(Clone, Copy, Debug)]
struct Place {
    frame: Bounds<Pixels>,
    selectable: bool,
    check: Bounds<Pixels>,
    icon: Bounds<Pixels>,
    caret: Bounds<Pixels>,
    words: Bounds<Pixels>,
    words_face: Face,
    keys: Bounds<Pixels>,
    keys_face: Face,
    detail: Bounds<Pixels>,
    detail_face: Face,
}

/// One drawn row: its entry's index in the menu, its row kind and where its parts go.
#[derive(Clone, Debug)]
struct Row {
    entry: usize,
    kind: NrMenuRowKind,
    place: Place,
}

impl Row {
    /// A row that shows text and does nothing.
    fn is_label(&self) -> bool {
        self.kind == NrMenuRowKind::Label
    }
}

/// A menu's rows as the core lays them out, and its drawn size in whole points.
struct Laid {
    rows: Vec<Row>,
    /// The core's layout width, before rounding to whole points.
    laid_width: f32,
    width: f32,
    height: f32,
    content_height: f32,
}

/// The row kind of `entry`.
fn kind_of(entry: &Entry) -> NrMenuRowKind {
    match entry.kind {
        Kind::Separator => NrMenuRowKind::Separator,
        Kind::Label => NrMenuRowKind::Label,
        Kind::Item => NrMenuRowKind::Item,
    }
}

/// A row's title, shortcut and detail line for the core; empty for a separator.
fn texts_of(entry: &Entry, kind: NrMenuRowKind) -> (CString, CString, CString) {
    let title = match kind {
        NrMenuRowKind::Separator => "",
        _ => &entry.title,
    };
    // An item has both; a label may show keys that pass through to what is under the menu.
    let (shortcut, detail) = match kind {
        NrMenuRowKind::Item => (entry.shortcut.as_str(), entry.detail.as_str()),
        NrMenuRowKind::Label => (entry.shortcut.as_str(), ""),
        _ => ("", ""),
    };
    (c(title), c(shortcut), c(detail))
}

fn lay_out(menu: &Menu) -> Laid {
    // Hidden entries belong only to the platform menu bar.
    let shown: Vec<(usize, &Entry)> = menu
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.hidden)
        .collect();
    let kinds: Vec<NrMenuRowKind> = shown.iter().map(|&(_, entry)| kind_of(entry)).collect();
    let item = |kind: NrMenuRowKind| kind == NrMenuRowKind::Item;
    let texts: Vec<(CString, CString, CString)> = shown
        .iter()
        .zip(&kinds)
        .map(|(&(_, entry), &kind)| texts_of(entry, kind))
        .collect();
    let items: Vec<NrMenuItem> = shown
        .iter()
        .zip(&kinds)
        .zip(&texts)
        .map(
            |((&(index, entry), &kind), (title, shortcut, detail))| NrMenuItem {
                kind,
                title: title.as_ptr(),
                shortcut: shortcut.as_ptr(),
                detail: detail.as_ptr(),
                image: item(kind) && (entry.icon.is_some() || entry.dot.is_some()),
                // A row that keeps the menu open toggles its tick in place, so it always reserves
                // the tick's space and its text never moves.
                ticked: item(kind) && (entry.check != Check::Off || entry.keeps_open),
                submenu: item(kind) && entry.submenu.is_some(),
                enabled: match kind {
                    NrMenuRowKind::Label => false,
                    _ => entry.enabled,
                },
                tight: menu.tight.contains(&index),
            },
        )
        .collect();
    let style = NrMenuStyle {
        min_width: menu.min_width,
        max_height: menu.max_height,
        icon_size: menu.icon_size,
        icon_gap: menu.icon_gap,
        quiet_labels: menu.quiet_labels,
    };
    // SAFETY: `items` holds `len` rows whose texts live in `texts` for the call.
    let placed = unsafe { nr_menu_layout(items.as_ptr(), items.len(), style) };
    let places = if placed.count == 0 {
        &[][..]
    } else {
        // SAFETY: the core lends `count` places until its next layout call.
        unsafe { std::slice::from_raw_parts(placed.places, placed.count) }
    };
    let rows = shown
        .iter()
        .zip(kinds)
        .zip(places)
        .map(|((&(index, _), kind), place)| Row {
            entry: index,
            kind,
            place: Place {
                frame: rect(&place.frame),
                selectable: place.selectable,
                check: rect(&place.check),
                icon: rect(&place.icon),
                caret: rect(&place.caret),
                words: rect(&place.words),
                words_face: Face::of(&place.words_font),
                keys: rect(&place.keys),
                keys_face: Face::of(&place.keys_font),
                detail: rect(&place.detail),
                detail_face: Face::of(&place.detail_font),
            },
        })
        .collect();
    Laid {
        rows,
        // An AppKit menu is its own window, sized in whole points, so place, draw and scale it
        // at that size.
        laid_width: placed.width as f32,
        width: placed.width.round() as f32,
        height: placed.height.round() as f32,
        content_height: placed.content_height as f32,
    }
}

/// Where a row's X goes (`Entry::ends`): 20 pt square, 4 pt from the far end, vertically
/// centred.
fn end_mark(frame: Bounds<Pixels>) -> Bounds<Pixels> {
    let middle = frame.center();
    crate::core::frame(
        f32::from(frame.origin.x + frame.size.width) - 24.0,
        f32::from(middle.y) - 10.0,
        20.0,
        20.0,
    )
}

/// One open menu.
struct Panel {
    /// Distinguishes this panel from one opened later in its place, for a pending submenu.
    serial: u64,
    menu: Menu,
    rows: Vec<Row>,
    width: f32,
    height: f32,
    content_height: f32,
    /// Resting top-left in the window.
    at: Point<Pixels>,
    /// The full slide-in offset, and the panel-local point it grows from.
    slide: Point<Pixels>,
    origin: Point<Pixels>,
    /// Its entrance or exit; `None` for a menu redrawn in place of another.
    motion: Option<Move>,
    lit: Option<usize>,
    /// Scroll offset, for a menu taller than its drawn height.
    offset: f32,
    /// The row in the parent menu that opened this submenu.
    from_row: Option<usize>,
    /// Whether the pointer is over it.
    pointed: bool,
    /// Whether the pointer is over the lit row's X (`Entry::ends`).
    over_end: bool,
}

impl Panel {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds {
            origin: self.at,
            size: size(px(self.width), px(self.height)),
        }
    }

    fn pose(&self) -> Pose {
        match &self.motion {
            Some(motion) => Pose::of(&motion.frame(), self.at, self.slide, self.origin),
            None => Pose::rest(self.at),
        }
    }

    /// A row's frame in the window, after scrolling.
    fn row_frame(&self, index: usize) -> Bounds<Pixels> {
        let frame = self.rows[index].place.frame;
        Bounds {
            origin: point(
                self.at.x + frame.origin.x,
                self.at.y + frame.origin.y - px(self.offset),
            ),
            size: frame.size,
        }
    }

    fn entry(&self, index: usize) -> &Entry {
        &self.menu.entries[self.rows[index].entry]
    }

    /// Whether `at` (window points) is on row `index`'s X, if it has one.
    fn on_end(&self, index: usize, at: Point<Pixels>) -> bool {
        !self.entry(index).ends.is_empty() && contains(&end_mark(self.row_frame(index)), at)
    }

    /// The selectable row under `at` (window points), with 1 pt of slack above and below.
    fn index_at(&self, at: Point<Pixels>) -> Option<usize> {
        let local = point(at.x - self.at.x, at.y - self.at.y + px(self.offset));
        self.rows.iter().position(|row| {
            let frame = crate::core::inset(row.place.frame, 0.0, -1.0);
            row.place.selectable && contains(&frame, local)
        })
    }

    /// The rows in the form `nr_menu_key` takes.
    fn key_rows(&self) -> Vec<NrMenuRow> {
        self.rows
            .iter()
            .map(|row| NrMenuRow {
                selectable: row.place.selectable,
                has_submenu: self.menu.entries[row.entry].submenu.is_some(),
            })
            .collect()
    }

    /// The pointer left the panel. A row whose submenu is open stays lit while the pointer is in
    /// the submenu.
    fn exited(&mut self) {
        if let Some(lit) = self.lit
            && nr_menu_stays_lit(self.entry(lit).submenu.is_some())
        {
            return;
        }
        self.lit = None;
    }
}

/// The open menus, deepest last, plus those leaving.
pub struct Menus {
    focus: FocusHandle,
    /// What had focus when the first menu opened.
    previous_focus: Option<FocusHandle>,
    panels: Vec<Panel>,
    leaving: Vec<Panel>,
    opened: u64,
    pending_submenu: Option<Task<()>>,
    run: Option<Run>,
    again: Option<Again>,
    /// The lit row's terminal, while one is lit that names one.
    peek: Option<Peek>,
}

impl Menus {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            previous_focus: None,
            panels: Vec::new(),
            leaving: Vec::new(),
            opened: 0,
            pending_submenu: None,
            run: None,
            again: None,
            peek: None,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.panels.is_empty()
    }

    /// Opens a context menu at `at` (window points), 2 pt right of the pointer. It flips left at
    /// the window's right edge and moves up at the bottom.
    pub fn open_context(
        &mut self,
        menu: Menu,
        at: Point<Pixels>,
        run: Run,
        again: Again,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show(menu, at, CONTEXT_GAP, false, run, again, window, cx);
    }

    /// Opens the menu with its top-left at `at`, hanging under its control (a select or dropdown)
    /// and sliding down into place.
    pub fn open_below(
        &mut self,
        menu: Menu,
        at: Point<Pixels>,
        run: Run,
        again: Again,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show(menu, at, 0.0, true, run, again, window, cx);
    }

    /// The drawn width of `menu`, for placing it by its right edge.
    pub fn width_of(menu: &Menu) -> f32 {
        lay_out(menu).laid_width
    }

    /// The drawn height of `menu`, for placing it by its bottom edge.
    pub fn height_of(menu: &Menu) -> f32 {
        lay_out(menu).height
    }

    #[allow(clippy::too_many_arguments)]
    fn show(
        &mut self,
        menu: Menu,
        at: Point<Pixels>,
        gap: f64,
        below: bool,
        run: Run,
        again: Again,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_all(window, cx);
        if menu.entries.iter().all(|entry| entry.hidden) {
            return;
        }
        self.run = Some(run);
        self.again = Some(again);
        let anchor = Bounds {
            origin: at,
            size: size(px(0.0), px(0.0)),
        };
        self.open(menu, anchor, gap, below, None, window);
        self.previous_focus = window.focused(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Opens `menu` `gap` away from `anchor` (window points), placed and animated by the core.
    fn open(
        &mut self,
        menu: Menu,
        anchor: Bounds<Pixels>,
        gap: f64,
        below: bool,
        from_row: Option<usize>,
        window: &Window,
    ) {
        let laid = lay_out(&menu);
        let placed = place(anchor, laid.width, laid.height, gap, below, window);
        self.opened += 1;
        self.panels.push(Panel {
            serial: self.opened,
            menu,
            rows: laid.rows,
            width: laid.width,
            height: laid.height,
            content_height: laid.content_height,
            at: placed.at,
            slide: placed.slide,
            origin: placed.origin,
            motion: Some(Move::entering(NrMotion::Popup)),
            lit: None,
            offset: 0.0,
            from_row,
            pointed: false,
            over_end: false,
        });
    }

    /// Closes every menu with a fade and shrink, and returns focus to where it was.
    pub fn close_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_submenu = None;
        self.peek = None;
        if self.panels.is_empty() {
            return;
        }
        let gone = std::mem::take(&mut self.panels);
        gone.into_iter().for_each(|panel| self.dismiss(panel));
        if self.focus.is_focused(window) {
            match self.previous_focus.take() {
                Some(previous) => window.focus(&previous, cx),
                None => window.blur(cx),
            }
        }
        cx.notify();
    }

    /// Starts `panel`'s exit, fading and shrinking about its top-left. It ignores the pointer
    /// while leaving.
    fn dismiss(&mut self, mut panel: Panel) {
        panel.at = panel.pose().at;
        panel.slide = Point::default();
        panel.origin = Point::default();
        panel.motion = Some(Move::leaving(NrMotion::Popup));
        self.leaving.push(panel);
    }

    /// The pointer is over row `index` of the menu at `depth`. Deeper menus close, and a row with
    /// a submenu opens it, immediately or, with `wait`, after the pointer rests briefly.
    fn hover(
        &mut self,
        index: Option<usize>,
        depth: usize,
        wait: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_submenu = None;
        if depth >= self.panels.len() {
            return;
        }
        let panel = &self.panels[depth];
        let wanted = index.and_then(|index| panel.entry(index).submenu.clone());
        let enabled = index.is_some_and(|index| panel.entry(index).enabled);
        let deeper_open = self.panels.len() > depth + 1;
        let deeper_is_rows =
            deeper_open && wanted.is_some() && self.panels[depth + 1].from_row == index;
        let answer = nr_menu_hover(
            index.is_some(),
            NrMenuRow {
                selectable: true,
                has_submenu: wanted.is_some(),
            },
            enabled,
            deeper_open,
            deeper_is_rows,
        );
        if answer.close_deeper {
            let deeper: Vec<Panel> = self.panels.drain(depth + 1..).collect();
            deeper.into_iter().for_each(|panel| self.dismiss(panel));
        }
        let (true, Some(index), Some(submenu)) = (answer.open, index, wanted) else {
            return;
        };
        let serial = self.panels[depth].serial;
        if !wait {
            self.open_submenu(depth, serial, index, submenu, window);
            return;
        }
        let delay = Duration::from_secs_f64(number(NrMotion::SubmenuDelay) / 1000.0);
        self.pending_submenu = Some(cx.spawn_in(window, async move |menus, cx| {
            cx.background_executor().timer(delay).await;
            let _ = menus.update_in(cx, |menus, window, cx| {
                menus.open_submenu(depth, serial, index, submenu, window);
                cx.notify();
            });
        }));
    }

    /// Opens row `index`'s submenu beside the row, aligned to its edge and top, if that menu is
    /// still the deepest.
    fn open_submenu(
        &mut self,
        depth: usize,
        serial: u64,
        index: usize,
        submenu: Menu,
        window: &Window,
    ) {
        if self.panels.len() != depth + 1 || self.panels[depth].serial != serial {
            return;
        }
        let row = self.panels[depth].row_frame(index);
        self.open(submenu, row, 0.0, false, Some(index), window);
    }

    fn choose(&mut self, depth: usize, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.panels[depth].entry(index).clone();
        let (Some(run), Some(again)) = (self.run.clone(), self.again.clone()) else {
            return;
        };
        let menus = cx.entity();
        if entry.keeps_open {
            // Run the entry, then redraw the menu as it now stands.
            window.defer(cx, move |window, cx| {
                run(&entry, window, cx);
                let now = again(cx);
                if let Some(menu) = now.holding(&entry.id).cloned() {
                    menus.update(cx, |menus, cx| menus.replace_last(menu, window, cx));
                }
            });
            return;
        }
        self.close_all(window, cx);
        window.defer(cx, move |window, cx| run(&entry, window, cx));
    }

    /// Replaces the deepest open menu with `menu`, keeping its top-left and scroll. Used after
    /// an entry that keeps the menu open.
    fn replace_last(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = self.panels.pop() else {
            return;
        };
        let laid = lay_out(&menu);
        let view_height = f64::from(window.viewport_size().height);
        let top = view_height - f64::from(old.at.y);
        let bottom = nr_menu_replacing_y(top, f64::from(laid.height), 0.0);
        self.panels.push(Panel {
            serial: old.serial,
            menu,
            rows: laid.rows,
            width: laid.width,
            height: laid.height,
            content_height: laid.content_height,
            at: point(old.at.x, px((view_height - bottom) as f32 - laid.height)),
            slide: Point::default(),
            origin: Point::default(),
            motion: None,
            lit: old.lit,
            offset: old.offset,
            from_row: old.from_row,
            pointed: old.pointed,
            over_end: false,
        });
        cx.notify();
    }

    /// Refreshes a lone open menu from the core (`Again`) in place. A menu whose rows haven't
    /// changed is left alone, and one with an open submenu waits, so nothing moves mid-choice.
    pub fn read_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ([panel], Some(again)) = (self.panels.as_slice(), self.again.clone()) else {
            return;
        };
        let now = again(cx);
        if now != panel.menu {
            self.replace_last(now, window, cx);
        }
    }

    /// Redraws the open menus, such as when a row's value arrives.
    pub fn redraw(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }

    /// The depth of the deepest open menu under `at`.
    fn panel_at(&self, at: Point<Pixels>) -> Option<usize> {
        self.panels
            .iter()
            .rposition(|panel| contains(&panel.bounds(), at))
    }

    fn mouse_move(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let over = self.panel_at(at);
        for (depth, panel) in self.panels.iter_mut().enumerate() {
            if panel.pointed && over != Some(depth) {
                panel.pointed = false;
                panel.exited();
            }
        }
        if let Some(depth) = over {
            let panel = &mut self.panels[depth];
            panel.pointed = true;
            let found = panel.index_at(at);
            panel.lit = found;
            panel.over_end = found.is_some_and(|index| panel.on_end(index, at));
            self.hover(found, depth, true, window, cx);
        }
        cx.notify();
    }

    /// A press anywhere but in a menu closes every menu.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.panel_at(event.position).is_none() {
            self.close_all(window, cx);
        }
    }

    fn mouse_up(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(depth) = self.panel_at(at) else {
            return;
        };
        let panel = &self.panels[depth];
        let Some(found) = panel.index_at(at) else {
            return;
        };
        if panel.on_end(found, at) {
            // The X runs its own entry, then the menu closes.
            let ending = Entry {
                id: panel.entry(found).ends.clone(),
                ..panel.entry(found).clone()
            };
            let Some(run) = self.run.clone() else { return };
            self.close_all(window, cx);
            window.defer(cx, move |window, cx| run(&ending, window, cx));
            return;
        }
        if panel.entry(found).submenu.is_none() {
            self.choose(depth, found, window, cx);
        }
    }

    /// Scrolls a menu taller than its drawn height (`nr_wheel_scrolled`).
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let Some(depth) = self.panel_at(event.position) else {
            return;
        };
        let (down, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (-f64::from(delta.y), NR_WHEEL_POINTS),
            ScrollDelta::Lines(delta) => (-f64::from(delta.y), NR_WHEEL_CLICKS),
        };
        let panel = &mut self.panels[depth];
        let moved = nr_wheel_scrolled(
            f64::from(panel.offset),
            down,
            unit,
            f64::from(panel.height),
            f64::from(panel.content_height),
        ) as f32;
        if moved != panel.offset {
            panel.offset = moved;
            cx.notify();
        }
    }

    /// The menu takes every key while open (`nr_menu_key`).
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        let press = crate::keys::press(&event.keystroke);
        self.key(press, window, cx);
    }

    /// Handles a key while a menu is open.
    pub fn key(&mut self, press: NrKeyPress, window: &mut Window, cx: &mut Context<Self>) {
        let Some(panel) = self.panels.last() else {
            return;
        };
        let depth = self.panels.len() - 1;
        let rows = panel.key_rows();
        let lit = panel.lit.map_or(-1, |lit| lit as i64);
        // SAFETY: `rows` holds `len` rows.
        let answer = unsafe { nr_menu_key(press, rows.as_ptr(), rows.len(), lit, depth) };
        let row = answer.row;
        match answer.intent {
            NrMenuIntent::CloseAll => self.close_all(window, cx),
            NrMenuIntent::Light => self.panels[depth].lit = Some(row),
            NrMenuIntent::OpenSubmenu => {
                self.hover(Some(row), depth, false, window, cx);
                if self.panels.len() > depth + 1 {
                    let opened = self.panels.last_mut().expect("a submenu opened");
                    let rows = opened.key_rows();
                    // SAFETY: `rows` holds `len` rows.
                    let first = unsafe { nr_menu_first_row(rows.as_ptr(), rows.len()) };
                    opened.lit = usize::try_from(first).ok();
                }
            }
            NrMenuIntent::Choose => self.choose(depth, row, window, cx),
            NrMenuIntent::CloseSubmenu => {
                let deepest = self.panels.pop().expect("a menu is open");
                self.dismiss(deepest);
            }
            NrMenuIntent::Nothing => {}
        }
        cx.notify();
    }

    /// The terminal file a lit row names (`Entry::peek`), with its menu's frame and the row's
    /// top in the window.
    fn peeked(&self) -> Option<(&str, Bounds<Pixels>, Pixels)> {
        self.panels.iter().find_map(|panel| {
            let lit = panel.lit?;
            let file = panel.entry(lit).peek.as_str();
            (!file.is_empty()).then(|| (file, panel.bounds(), panel.row_frame(lit).origin.y))
        })
    }

    /// Keeps the peek on the terminal the lit row names: read when the row is lit, then again
    /// every `PEEK_EVERY` while it stays lit, and dropped when no such row is lit.
    fn follow_peek(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = self.peeked().map(|(file, ..)| file.to_owned());
        if wanted.as_deref() == self.peek.as_ref().map(|peek| peek.file.as_str()) {
            return;
        }
        self.peek = wanted.map(|file| Peek {
            lines: peek::shown(&file),
            file,
            _follow: cx.spawn_in(window, async move |menus, cx| {
                loop {
                    cx.background_executor().timer(PEEK_EVERY).await;
                    let read = menus.update(cx, |menus, cx| {
                        let Some(peek) = &mut menus.peek else { return };
                        let now = peek::shown(&peek.file);
                        if now != peek.lines {
                            peek.lines = now;
                            cx.notify();
                        }
                    });
                    if read.is_err() {
                        return;
                    }
                }
            }),
        });
    }

    /// Draws the peek on the menu's surface, its lines in the muted colour. It takes no pointer:
    /// moving onto it leaves the row, which takes it away.
    fn draw_peek(&self, window: &mut Window, cx: &mut App) {
        let (Some(peek), Some((_, menu, row_top))) = (&self.peek, self.peeked()) else {
            return;
        };
        let view = window.viewport_size();
        let menu = northroom_core::board::Frame {
            x: f64::from(menu.origin.x),
            y: f64::from(menu.origin.y),
            width: f64::from(menu.size.width),
            height: f64::from(menu.size.height),
        };
        let Some(panel) = peek::place(
            peek.lines.len(),
            menu,
            f64::from(row_top),
            (f64::from(view.width), f64::from(view.height)),
        ) else {
            return;
        };
        let bounds = |frame: northroom_core::board::Frame| {
            crate::core::frame(
                frame.x as f32,
                frame.y as f32,
                frame.width as f32,
                frame.height as f32,
            )
        };
        let pose = Pose::rest(Point::default());
        let whole = bounds(panel);
        draw_surface(pose, whole, window);
        let face = Face::mono(peek::FONT.size as f32);
        let ink = color(Token::MutedForeground);
        let clip = crate::core::inset(whole, 1.0, 1.0);
        window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
            for (index, line) in peek.lines.iter().enumerate() {
                let frame = bounds(peek::line_frame(panel, index));
                let max_width = f32::from(frame.size.width);
                pose.line(window, cx, line, face, ink, frame, max_width);
            }
        });
    }

    // For checks.

    /// The open menus, back to front, as row titles ("-" for a separator).
    pub fn open_rows(&self) -> Vec<Vec<String>> {
        self.panels
            .iter()
            .map(|panel| {
                panel
                    .rows
                    .iter()
                    .map(|row| {
                        let entry = &panel.menu.entries[row.entry];
                        match row.kind {
                            NrMenuRowKind::Separator => "-".to_owned(),
                            _ => entry.title.clone(),
                        }
                    })
                    .collect()
            })
            .collect()
    }

    /// The frame of every open menu in the window.
    pub fn frames(&self) -> Vec<Bounds<Pixels>> {
        self.panels.iter().map(Panel::bounds).collect()
    }

    /// The deepest menu's scroll offset, visible height and content height.
    pub fn scrolled(&self) -> Option<(f32, f32, f32)> {
        self.panels
            .last()
            .map(|panel| (panel.offset, panel.height, panel.content_height))
    }

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leaving
            .retain(|panel| !panel.motion.as_ref().is_some_and(Move::gone));
        let moving = self
            .panels
            .iter()
            .chain(&self.leaving)
            .any(|panel| panel.motion.as_ref().is_some_and(Move::runs));
        for panel in self.leaving.iter().chain(&self.panels) {
            draw_panel(panel, window, cx);
        }
        self.follow_peek(window, cx);
        self.draw_peek(window, cx);
        if moving {
            window.request_animation_frame();
        }
        Self::listen_to_pointer(window, cx);
    }

    /// Registers this frame's pointer handlers: moves and presses anywhere, and releases and
    /// scrolls over an open menu, which stop there.
    fn listen_to_pointer(window: &mut Window, cx: &mut Context<Self>) {
        let menus = cx.entity();
        window.on_mouse_event({
            let menus = menus.clone();
            move |event: &MouseMoveEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    menus.update(cx, |menus, cx| menus.mouse_move(event.position, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let menus = menus.clone();
            move |event: &MouseDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble {
                    cx.stop_propagation();
                    menus.update(cx, |menus, cx| menus.mouse_down(event, window, cx));
                }
            }
        });
        window.on_mouse_event({
            let menus = menus.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    Self::release_over_menu(&menus, event.position, window, cx);
                }
            }
        });
        window.on_mouse_event(move |event: &ScrollEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble && menus.read(cx).panel_at(event.position).is_some() {
                cx.stop_propagation();
                menus.update(cx, |menus, cx| menus.scroll(event, cx));
            }
        });
    }

    /// A left-button release over an open menu stops at the menu.
    fn release_over_menu(
        menus: &Entity<Self>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if menus.read(cx).panel_at(at).is_none() {
            return;
        }
        cx.stop_propagation();
        menus.update(cx, |menus, cx| menus.mouse_up(at, window, cx));
    }
}

/// Draws a menu on the popover surface: 1 px border, 10 px corners, 4 px padding, 24 pt rows
/// 2 pt apart, 12 px type, muted 14 px glyphs, and the hovered row on 8% of the text colour. It
/// appears and leaves as one group (`crate::layer::group`).
fn draw_panel(panel: &Panel, window: &mut Window, cx: &mut App) {
    let pose = panel.pose();
    crate::layer::group(window, cx, pose.opacity, |window, cx| {
        draw_panel_at(
            panel,
            Pose {
                opacity: 1.0,
                ..pose
            },
            window,
            cx,
        )
    });
}

/// Paints the surface menus sit on over panel-local `whole`: the popover colour, a 1 px border
/// and 10 px corners.
fn draw_surface(pose: Pose, whole: Bounds<Pixels>, window: &mut Window) {
    window.paint_quad(gpui::quad(
        pose.rect(whole),
        px(pose.length(10.0)),
        pose.color(color(Token::Popover)),
        px(pose.length(1.0)),
        pose.color(color(Token::Border)),
        gpui::BorderStyle::Solid,
    ));
}

fn draw_panel_at(panel: &Panel, pose: Pose, window: &mut Window, cx: &mut App) {
    let whole = Bounds {
        origin: Point::default(),
        size: size(px(panel.width), px(panel.height)),
    };
    draw_surface(pose, whole, window);
    let painter = RowPainter {
        panel,
        pose,
        text: color(Token::Foreground),
        muted: color(Token::MutedForeground),
        destructive: color(Token::Destructive),
    };
    // Rows, scrolled and clipped inside the border.
    let clip = pose.rect(crate::core::inset(whole, 1.0, 1.0));
    window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
        for (index, row) in panel.rows.iter().enumerate() {
            painter.draw_row(window, cx, index, row);
        }
    });
}

/// Draws one panel's rows with its pose and text colours.
struct RowPainter<'a> {
    panel: &'a Panel,
    pose: Pose,
    text: Hsla,
    muted: Hsla,
    destructive: Hsla,
}

/// One row being drawn, scrolled, with its text colour. Everything is faded by `fade` while the
/// row is disabled.
struct DrawnRow<'a> {
    row: &'a Row,
    entry: &'a Entry,
    place: Place,
    ink: Hsla,
    fade: f32,
}

impl RowPainter<'_> {
    /// Fills panel-local `frame` with `color` and corner `radius`.
    fn fill(&self, window: &mut Window, frame: Bounds<Pixels>, color: Hsla, radius: f32) {
        let pose = &self.pose;
        window.paint_quad(
            gpui::fill(pose.rect(frame), pose.color(color)).corner_radii(px(pose.length(radius))),
        );
    }

    /// Draws `words` in the line box `frame`, truncated to its width.
    fn draw_words(
        &self,
        window: &mut Window,
        cx: &mut App,
        words: &str,
        face: Face,
        ink: Hsla,
        frame: Bounds<Pixels>,
    ) {
        let max_width = f32::from(frame.size.width);
        self.pose
            .line(window, cx, words, face, ink, frame, max_width);
    }

    /// A row's parts, after scrolling.
    fn scrolled(&self, place: &Place) -> Place {
        let scrolled = |frame: Bounds<Pixels>| crate::core::offset(frame, 0.0, -self.panel.offset);
        Place {
            frame: scrolled(place.frame),
            check: scrolled(place.check),
            icon: scrolled(place.icon),
            caret: scrolled(place.caret),
            words: scrolled(place.words),
            keys: scrolled(place.keys),
            detail: scrolled(place.detail),
            ..*place
        }
    }

    /// A row's text colour: the destructive colour or the normal text colour.
    fn ink_of(&self, entry: &Entry) -> Hsla {
        if entry.destructive {
            self.destructive
        } else {
            self.text
        }
    }

    fn draw_row(&self, window: &mut Window, cx: &mut App, index: usize, row: &Row) {
        let entry = &self.panel.menu.entries[row.entry];
        let place = self.scrolled(&row.place);
        if row.kind == NrMenuRowKind::Separator {
            self.fill(window, place.frame, color(Token::Border), 0.0);
            return;
        }
        let enabled = row.is_label() || entry.enabled;
        let fade = if enabled { 1.0 } else { 0.5 };
        if self.panel.lit == Some(index) {
            self.draw_light(window, entry, place.frame);
        }
        let drawn = DrawnRow {
            row,
            entry,
            place,
            ink: self.ink_of(entry).alpha(fade),
            fade,
        };
        self.draw_check(window, cx, &drawn);
        match row.kind {
            NrMenuRowKind::Item => {
                self.draw_item_marks(window, cx, &drawn);
                self.draw_title(window, cx, &drawn);
                if self.panel.lit == Some(index) && !entry.ends.is_empty() {
                    self.draw_end(window, cx, place.frame);
                }
            }
            NrMenuRowKind::Label => {
                self.draw_keys(window, cx, &drawn);
                self.draw_title(window, cx, &drawn);
            }
            // Already drawn above.
            NrMenuRowKind::Separator => {}
        }
    }

    /// The lit row's X at its far end, with a highlight behind it while hovered.
    fn draw_end(&self, window: &mut Window, cx: &mut App, frame: Bounds<Pixels>) {
        let mark = end_mark(frame);
        let ink = if self.panel.over_end {
            self.fill(window, mark, mixed(Token::Foreground, 0.14), 10.0);
            self.text
        } else {
            self.muted
        };
        icons::draw(
            window,
            cx,
            Glyph::Close,
            self.pose.rect(crate::core::inset(mark, 4.0, 4.0)),
            self.pose.color(ink),
        );
    }

    /// The hovered row's highlight, in the destructive colour or the text colour.
    fn draw_light(&self, window: &mut Window, entry: &Entry, frame: Bounds<Pixels>) {
        let strength = nr_menu_light(entry.destructive, nr_appearance_is_dark()) as f32;
        let light = if entry.destructive {
            self.destructive.alpha(strength)
        } else {
            mixed(Token::Foreground, strength)
        };
        self.fill(window, frame, light, 6.0);
    }

    /// A tick, or a dash when mixed, if the row has room for one.
    fn draw_check(&self, window: &mut Window, cx: &mut App, drawn: &DrawnRow) {
        let check = drawn.place.check;
        if crate::core::is_empty(&check) {
            return;
        }
        match drawn.entry.check {
            Check::On => icons::draw(
                window,
                cx,
                Glyph::Check,
                self.pose.rect(check),
                self.pose.color(drawn.ink),
            ),
            Check::Mixed => {
                let dash = crate::core::frame(
                    f32::from(check.origin.x) + 2.0,
                    f32::from(check.center().y) - 0.75,
                    8.0,
                    1.5,
                );
                self.fill(window, dash, drawn.ink, 0.0);
            }
            Check::Off => {}
        }
    }

    /// An item's glyph, its submenu's caret and its shortcut.
    fn draw_item_marks(&self, window: &mut Window, cx: &mut App, drawn: &DrawnRow) {
        let (entry, place) = (drawn.entry, &drawn.place);
        if let Some(icon) = &entry.icon {
            // Glyphs are muted except on destructive rows.
            let glyph = if entry.destructive {
                self.destructive
            } else {
                self.muted
            }
            .alpha(drawn.fade);
            icons::draw(
                window,
                cx,
                *icon,
                self.pose.rect(place.icon),
                self.pose.color(glyph),
            );
        }
        if let Some(dot) = &entry.dot {
            // A session's 7 pt dot, centred in the glyph's place.
            let middle = place.icon.center();
            let at = crate::core::frame(
                f32::from(middle.x) - 3.5,
                f32::from(middle.y) - 3.5,
                7.0,
                7.0,
            );
            let ink = crate::theme::faded(color(dot.colour), dot.fade * f64::from(drawn.fade));
            crate::signs::paint(
                window,
                cx,
                dot.sign.name(),
                self.pose.rect(at),
                self.pose.color(ink),
            );
        }
        if !entry.detail.is_empty() {
            self.draw_words(
                window,
                cx,
                &entry.detail,
                place.detail_face,
                self.muted.alpha(drawn.fade),
                place.detail,
            );
        }
        if entry.submenu.is_some() {
            icons::draw(
                window,
                cx,
                Glyph::CaretRight,
                self.pose.rect(place.caret),
                self.pose.color(self.muted.alpha(drawn.fade)),
            );
        }
        self.draw_keys(window, cx, drawn);
    }

    /// The row's shortcut at its far edge, if any.
    fn draw_keys(&self, window: &mut Window, cx: &mut App, drawn: &DrawnRow) {
        let (entry, place) = (drawn.entry, &drawn.place);
        if entry.shortcut.is_empty() {
            return;
        }
        self.draw_words(
            window,
            cx,
            &entry.shortcut,
            place.keys_face,
            self.muted.alpha(drawn.fade),
            place.keys,
        );
    }

    /// An item's or label's title.
    fn draw_title(&self, window: &mut Window, cx: &mut App, drawn: &DrawnRow) {
        // Quiet labels (the page's block menu) use the muted colour.
        let quiet = drawn.row.is_label() && self.panel.menu.quiet_labels;
        let ink = if quiet { self.muted } else { drawn.ink };
        let place = &drawn.place;
        self.draw_words(
            window,
            cx,
            &drawn.entry.title,
            place.words_face,
            ink,
            place.words,
        );
    }
}

impl Render for Menus {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.panels.is_empty() && self.leaving.is_empty() {
            return div().into_any_element();
        }
        let view = window.viewport_size();
        let this = cx.entity();
        let frames: Vec<Bounds<Pixels>> = self.frames();
        let open = self.is_open();
        let layer = div()
            .id("menus")
            .absolute()
            .top_0()
            .left_0()
            .w(view.width)
            .h(view.height)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                canvas(
                    move |_, window, _| {
                        // Open menus block the pointer from what is under them; leaving menus
                        // don't.
                        for frame in frames.iter().filter(|_| open) {
                            window.insert_hitbox(*frame, HitboxBehavior::BlockMouse);
                        }
                    },
                    move |_, (), window, cx| {
                        this.update(cx, |menus, cx| {
                            if menus.is_open() {
                                menus.paint(window, cx);
                            } else {
                                menus.paint_leaving(window, cx);
                            }
                        })
                    },
                )
                .size_full(),
            );
        crate::popover::layer(layer, Layer::Menu)
    }
}

impl Menus {
    /// Draws only leaving menus, which take no pointer or key input.
    fn paint_leaving(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leaving
            .retain(|panel| !panel.motion.as_ref().is_some_and(Move::gone));
        for panel in &self.leaving {
            draw_panel(panel, window, cx);
        }
        if self
            .leaving
            .iter()
            .any(|panel| panel.motion.as_ref().is_some_and(Move::runs))
        {
            window.request_animation_frame();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: Kind) -> Entry {
        Entry {
            kind,
            ..Entry::item("", "")
        }
    }

    #[test]
    fn each_entry_is_laid_out_as_the_row_kind_the_core_names() {
        assert_eq!(kind_of(&entry(Kind::Item)), NrMenuRowKind::Item);
        assert_eq!(kind_of(&entry(Kind::Separator)), NrMenuRowKind::Separator);
        assert_eq!(kind_of(&entry(Kind::Label)), NrMenuRowKind::Label);
    }
}
