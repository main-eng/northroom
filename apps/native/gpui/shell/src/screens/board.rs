//! The folder as a board. The core places the cards (`nr_board_place`), supplies the marks each
//! is drawn from (`nr_board_card`) and poses the animated folder art. Presses, keys, hover and
//! the wheel go to the core (`nr_board_press`, `nr_board_key`, `nr_board_hover`,
//! `nr_board_wheel`), as do skimming a playable card and pressing its play button
//! (`nr_board_skim`, `nr_board_play_at`). It registers a check step for its scroll (`register`).

use super::{Ask, Folder, Open, Screen, folder_menu, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use crate::script::{Answer, Pointer, Route, Step, Table, workspace};
use gpui::{
    App, Bounds, Context, Corners, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, Hsla,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, PathBuilder, Pixels, Point, Render, RenderImage, Rgba, ScrollDelta, ScrollEvent,
    Styled, Window, canvas, linear_color_stop, linear_gradient, point, prelude::*, px,
};
use kit::core::{c, contains, empty_rect, inset, nr_rect, rect, text, zeroed};
use kit::draw::{cover, cover_from_top, oval, rounded, stroke};
use kit::icons;
use kit::lists::rename::Said;
use kit::lists::{DriveCommands, chip, thumbs};
use kit::motion::now;
use kit::nr::*;
use kit::text::{browser_baseline, draw_line, draw_words, metrics};
use kit::theme::{Face, Token, color, faded, mixed, named};
use std::rc::Rc;
use std::sync::Arc;

/// A card's corner radius.
const CARD_RADIUS: f32 = 12.0;

/// The board's key context, alongside the drive views' (`kit::lists::CONTEXT`).
const CONTEXT: &str = "KitBoard";

pub struct Card {
    pub id: String,
    pub title: String,
}

pub struct Board {
    focus: FocusHandle,
    /// Where the window placed the board.
    frame: Bounds<Pixels>,
    cards: Vec<Card>,
    frames: Vec<Bounds<Pixels>>,
    /// The vertical scroll offset, and the board's height.
    scroll: f32,
    height: f32,
    /// The width the board was last placed at; it stages once, at its first width.
    placed_width: f32,
    staged: bool,
    /// Folder art is animating to a new pose, drawn frame by frame until it arrives.
    moving: bool,
    /// The pressed card and where, until release or until the press becomes a drag.
    pressed: Option<(usize, Point<Pixels>)>,
}

impl Board {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        // A newly shown board starts from the core's first pose.
        nr_board_reset();
        let mut board = Self {
            focus: cx.focus_handle(),
            frame: Bounds::default(),
            cards: Vec::new(),
            frames: Vec::new(),
            scroll: 0.0,
            height: 0.0,
            placed_width: 0.0,
            staged: false,
            moving: false,
            pressed: None,
        };
        board.read_cards();
        board
    }

    /// Reads the folder's cards again (`nr_board_load`).
    fn read_cards(&mut self) {
        let count = nr_board_load();
        self.cards = (0..count)
            .filter_map(|index| {
                let mut row: NrFileRow = zeroed();
                // SAFETY: a row to fill.
                unsafe { nr_board_row(index, &mut row) }.then(|| Card {
                    id: text(&row.id),
                    title: text(&row.title),
                })
            })
            .collect();
        self.placed_width = 0.0;
    }

    /// Places the cards on a board `width` wide with `visible` height in view.
    fn lay_out(&mut self, width: f32, visible: f32, now: f64) {
        if width <= 0.0 {
            return;
        }
        if !self.staged {
            self.staged = true;
            let staged = nr_board_stage();
            if staged.folders {
                self.move_folders(now);
            }
        }
        if width == self.placed_width && !self.frames.is_empty() {
            return;
        }
        self.placed_width = width;
        self.height = nr_board_place(f64::from(width), f64::from(visible)) as f32;
        self.frames = (0..self.cards.len())
            .map(|index| {
                let mut frame = empty_rect();
                // SAFETY: a rect to fill.
                unsafe { nr_board_frame(index, &mut frame) };
                rect(&frame)
            })
            .collect();
        self.scroll = self.scroll.clamp(0.0, (self.height - visible).max(0.0));
    }

    /// Converts window point `at` to board coordinates at the current scroll.
    fn inside(&self, at: Point<Pixels>) -> Point<Pixels> {
        point(
            at.x - self.frame.origin.x,
            at.y - self.frame.origin.y + px(self.scroll),
        )
    }

    /// The window frame of the card titled `title`.
    fn card_frame(&self, title: &str) -> Option<Bounds<Pixels>> {
        let index = self.cards.iter().position(|card| card.title == title)?;
        let frame = *self.frames.get(index)?;
        Some(Bounds {
            origin: frame.origin
                + point(self.frame.origin.x, self.frame.origin.y - px(self.scroll)),
            size: frame.size,
        })
    }

    /// The card under `at` in the window.
    fn card_at(&self, at: Point<Pixels>) -> Option<usize> {
        if !contains(&self.frame, at) {
            return None;
        }
        let inside = self.inside(at);
        self.frames
            .iter()
            .position(|frame| !kit::core::is_empty(frame) && contains(frame, inside))
    }

    /// Starts each folder's art toward the core's current pose.
    fn move_folders(&mut self, now: f64) {
        self.moving = nr_board_folders_retarget(now, false);
    }

    /// Opens card `card`'s folder or file.
    fn open(&self, card: usize, cx: &mut Context<Self>) {
        let id = c(&self.cards[card].id);
        // SAFETY: a NUL-terminated id.
        unsafe { nr_folder_open(id.as_ptr()) };
        cx.emit(Ask::Changed);
    }

    /// Opens and plays card `card`'s video, as its play button does.
    fn play(&self, card: usize, cx: &mut Context<Self>) {
        nr_board_skim(-1, 0.0);
        let id = c(&self.cards[card].id);
        // SAFETY: a NUL-terminated id.
        unsafe { nr_folder_play(id.as_ptr()) };
        cx.emit(Ask::Changed);
    }

    /// The card whose play button is under `at` in the window (`nr_board_play_at`).
    fn play_at(&self, at: Point<Pixels>) -> Option<usize> {
        if !contains(&self.frame, at) {
            return None;
        }
        let inside = self.inside(at);
        usize::try_from(nr_board_play_at(f64::from(inside.x), f64::from(inside.y))).ok()
    }

    /// Tells the core which card the pointer at `at` is over and how far across (`None` when off
    /// the cards), so a playable card shows that moment. Returns whether a card needs redrawing.
    fn skim(&self, card: Option<usize>, at: Point<Pixels>) -> bool {
        let across = card
            .and_then(|card| self.frames.get(card))
            .map_or(0.0, |frame| {
                let inside = self.inside(at);
                f64::from((inside.x - frame.origin.x) / frame.size.width)
            });
        nr_board_skim(card.map_or(-1, |card| card as i64), across)
    }

    // MARK: Pointer and keys

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button == MouseButton::Right
            || (event.button == MouseButton::Left && event.modifiers.control)
        {
            self.menu(event.position, cx);
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        window.focus(&self.focus, cx);
        if let Some(card) = self.play_at(event.position) {
            self.pressed = None;
            self.play(card, cx);
            return;
        }
        let card = self.card_at(event.position);
        let asked = nr_board_press(
            card.map_or(-1, |card| card as i64),
            event.click_count as u32,
        );
        self.pressed = None;
        match (asked, card) {
            (NrBoardPress::Open, Some(card)) => self.open(card, cx),
            (NrBoardPress::Pressed, Some(card)) => self.pressed = Some((card, event.position)),
            _ => {}
        }
        cx.notify();
    }

    /// The press passed the drag threshold. The selected cards join the drag
    /// (`nr_board_pick_up`) but stay in place, and a chip with their names follows the pointer.
    fn pick_up(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.pressed.take().is_none() {
            return;
        }
        let carried = text(&nr_board_pick_up());
        let ids: Vec<String> = carried.lines().map(str::to_owned).collect();
        if ids.is_empty() {
            return;
        }
        let titles: Vec<String> = ids
            .iter()
            .filter_map(|id| self.cards.iter().find(|card| &card.id == id))
            .map(|card| card.title.clone())
            .collect();
        chip::begin(chip::BOARD, ids, &titles, at);
        self.drag_over(at, cx);
        window.refresh();
    }

    /// The dragged cards are over window point `at`; the card there may accept them
    /// (`nr_board_drag_over`).
    fn drag_over(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(ids) = chip::carried() else {
            return;
        };
        let card = self.card_at(at).map_or(-1, |card| card as i64);
        let ids = c(&ids.join("\n"));
        // SAFETY: a NUL-terminated list.
        let folders = unsafe { nr_board_drag_over(ids.as_ptr(), card) };
        self.dragged(folders, cx);
    }

    /// The dragged cards left the board, so no card accepts them.
    fn leave(&mut self, cx: &mut Context<Self>) {
        self.dragged(nr_board_drag_leave(), cx);
    }

    fn dragged(&mut self, folders: bool, cx: &mut Context<Self>) {
        if folders {
            self.move_folders(now());
        }
        cx.notify();
    }

    /// Drops the dragged cards where they were last dragged (`nr_board_drop`): onto the card
    /// under them, or onto empty space, where nothing moves.
    fn drop(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let joined = c(&ids.join("\n"));
        // SAFETY: a NUL-terminated list.
        let dropped = unsafe { nr_board_drop(joined.as_ptr()) };
        if dropped.folders {
            self.move_folders(now());
        }
        // Files from outside the app arrive through the window's drop, not a card drag.
        if dropped.kind != NrBoardDropKind::Card {
            return;
        }
        let target = c(&self.cards[dropped.card as usize].id);
        // SAFETY: NUL-terminated strings.
        let said = Said::of(&unsafe {
            nr_folder_drop(nr_shown_column(), joined.as_ptr(), target.as_ptr())
        });
        cx.emit(Ask::Said(said));
    }

    /// Ends the drag, dropped or not.
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if nr_board_drag_leave() {
            self.move_folders(now());
        }
        cx.emit(Ask::Changed);
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, over: bool, cx: &mut Context<Self>) {
        self.pressed = None;
        let Some(drag) = chip::current().filter(|drag| drag.from == chip::BOARD) else {
            cx.notify();
            return;
        };
        chip::end();
        if over {
            self.drag_over(event.position, cx);
            self.drop(&drag.ids, cx);
        } else {
            self.leave(cx);
        }
        self.end_drag(cx);
    }

    /// A right click opens the menu for what is under it: a card's menu for that card alone
    /// (the selection is unchanged and no ring is drawn), or the board's own.
    fn menu(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let column = nr_shown_column();
        let Some(card) = self.card_at(at) else {
            cx.emit(Ask::FileMenu {
                which: NrMenu::Background,
                again: folder_menu(NrMenu::Background, column, "", &[]),
                at,
                folder: Folder { column, open: None },
            });
            return;
        };
        let id = self.cards[card].id.clone();
        let again = folder_menu(NrMenu::Card, column, &id, &[]);
        let open: Open = Rc::new(move |_, _| {
            let id = c(&id);
            // SAFETY: a NUL-terminated id.
            unsafe { nr_folder_open(id.as_ptr()) };
        });
        cx.emit(Ask::FileMenu {
            which: NrMenu::Card,
            again,
            at,
            folder: Folder {
                column,
                open: Some(open),
            },
        });
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if chip::current().is_some_and(|drag| drag.from == chip::BOARD) {
            chip::follow(event.position);
            if over {
                self.drag_over(event.position, cx);
            } else {
                self.leave(cx);
            }
            // Outside the window, the drag continues as a platform drag of the files, and the
            // board restores the cards.
            if chip::carry_out(event.position, window, cx) {
                self.end_drag(cx);
            }
            window.refresh();
            return;
        }
        if self.move_pressed_card(event, window, cx) {
            return;
        }
        let card = over.then(|| self.card_at(event.position)).flatten();
        if nr_board_hover(card.map_or(-1, |card| card as i64)) {
            self.move_folders(now());
            cx.notify();
        }
        // A playable card shows the moment under the pointer, and its play button.
        if self.skim(card, event.position) {
            cx.notify();
        }
    }

    /// A held card consumes the move; crossing the platform's threshold picks it up.
    fn move_pressed_card(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.pressed_button != Some(MouseButton::Left) {
            return false;
        }
        let Some((_, from)) = self.pressed else {
            return false;
        };
        let moved = event.position - from;
        if nr_drag_starts(
            f64::from(moved.x),
            f64::from(moved.y),
            f64::from(window.scale_factor()),
        ) {
            self.pick_up(event.position, window, cx);
        }
        true
    }

    /// The wheel zooms in steps with ⌘ or Control held; otherwise the board scrolls.
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let (down, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (-f64::from(delta.y), NR_WHEEL_POINTS),
            ScrollDelta::Lines(delta) => (-f64::from(delta.y), NR_WHEEL_CLICKS),
        };
        let width = f64::from(self.frame.size.width);
        let zoomed = nr_board_wheel(
            down,
            unit,
            event.modifiers.control,
            event.modifiers.platform,
            width,
        );
        if zoomed < 0 {
            let shown = f32::from(self.frame.size.height);
            let extent = self.height.max(shown);
            let next = if unit == NR_WHEEL_POINTS {
                Some(f64::from(self.scroll) + down)
            } else {
                northroom_core::scroll::route::wheel_scroll(
                    f64::from(self.scroll),
                    down,
                    unit,
                    f64::from(shown),
                    f64::from(extent),
                )
            };
            if let Some(next) = next {
                self.scroll = (next as f32).clamp(0.0, (extent - shown).max(0.0));
            }
        } else if zoomed > 0 {
            self.placed_width = 0.0;
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        if event.keystroke.key == "escape"
            && chip::current().is_some_and(|drag| drag.from == chip::BOARD)
        {
            // Escape cancels a drag in progress; nothing moves.
            chip::end();
            self.leave(cx);
            self.end_drag(cx);
            window.refresh();
            cx.stop_propagation();
            return;
        }
        let asked = nr_board_key(kit::keys::press(&event.keystroke));
        match asked.answer {
            NrBoardKeyAnswer::Open => self.open(asked.card as usize, cx),
            // No confirmation, since ⌘Z undoes it.
            NrBoardKeyAnswer::Trash => {
                let said = kit::lists::edit_files(NrDriveEdit::Trash, nr_shown_column() as i64);
                cx.emit(Ask::Said(said));
            }
            NrBoardKeyAnswer::PassOn | NrBoardKeyAnswer::Taken | NrBoardKeyAnswer::Leave => {}
        }
        cx.emit(Ask::Changed);
        cx.notify();
    }

    // MARK: Drawing

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        let frame = self.frame;
        self.lay_out(
            f32::from(frame.size.width),
            f32::from(frame.size.height),
            now,
        );
        if self.moving {
            self.moving = nr_board_folders_step(now);
        }
        let origin = point(frame.origin.x, frame.origin.y - px(self.scroll));
        let focus = nr_board_focus();
        let focused = self.focus.is_focused(window);
        window.with_content_mask(Some(gpui::ContentMask { bounds: frame }), |window| {
            self.paint_cards(window, cx, origin, &focus, now);
            if focused {
                self.paint_focus_ring(window, origin, focus.ring);
            }
        });
        // Folder art is animating.
        if self.moving {
            window.request_animation_frame();
        }
        self.listen(hitbox, window, cx);
    }

    /// Paints the cards that intersect the board, with the content origin at `origin`.
    fn paint_cards(
        &self,
        window: &mut Window,
        cx: &mut App,
        origin: Point<Pixels>,
        focus: &NrBoardFocus,
        now: f64,
    ) {
        let frame = self.frame;
        for (index, card) in self.frames.iter().enumerate() {
            if kit::core::is_empty(card) || index >= self.cards.len() {
                continue;
            }
            let placed = Bounds {
                origin: card.origin + origin,
                size: card.size,
            };
            if placed.origin.y > frame.origin.y + frame.size.height
                || placed.origin.y + placed.size.height < frame.origin.y
            {
                continue;
            }
            paint_card(window, cx, index, &self.cards[index].id, placed, focus, now);
        }
    }

    /// The focus ring around card `ring` (none when -1): 2 px in the ring colour at 50% opacity.
    fn paint_focus_ring(&self, window: &mut Window, origin: Point<Pixels>, ring: i64) {
        let Some(card) = usize::try_from(ring)
            .ok()
            .and_then(|ring| self.frames.get(ring))
            .filter(|card| !kit::core::is_empty(card))
        else {
            return;
        };
        let placed = Bounds {
            origin: card.origin + origin,
            size: card.size,
        };
        stroke(
            window,
            inset(placed, -1.0, -1.0),
            mixed(Token::Ring, 0.5),
            13.0,
            2.0,
        );
    }

    /// Registers pointer handlers while the board is drawn.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let board = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            board.update(cx, |board, cx| board.mouse_down(event, window, cx));
        });
        let (board, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
            let hovered = over.is_hovered(window);
            board.update(cx, |board, cx| board.mouse_move(event, hovered, window, cx));
        });
        let (board, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseUpEvent, window, cx| {
            if event.button == MouseButton::Left {
                let hovered = over.is_hovered(window);
                board.update(cx, |board, cx| board.mouse_up(event, hovered, cx));
            }
        });
        let board = entity;
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            board.update(cx, |board, cx| board.scroll(event, cx));
        });
    }
}

impl EventEmitter<Ask> for Board {}

impl Screen for Board {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.cards.is_empty() {
            nr_board_reset();
        }
        self.read_cards();
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        probe.folder = NrFolderView::Board;
        if self.focus.is_focused(window) {
            probe.keys = "BoardCanvas".to_owned();
        }
    }

    fn point_of(&self, title: &str, _: &App) -> Option<Point<Pixels>> {
        self.card_frame(title).map(|frame| frame.center())
    }
}

impl Render for Board {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let board = cx.entity();
        let commanded = board.clone();
        kit::controls::placed("board", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .key_context(format!("{} {CONTEXT}", kit::lists::CONTEXT).as_str())
            // Drive commands act on the selected cards, which the board's column holds.
            .take_drive_commands(
                &self.focus,
                || nr_shown_column() as i64,
                Rc::new(move |said, _, cx| commanded.update(cx, |_, cx| cx.emit(Ask::Said(said)))),
            )
            .on_key_down(cx.listener(Self::key_down))
            // Files dropped from outside the app are copied into the shown folder.
            .on_drop(cx.listener(|_, dropped: &gpui::ExternalPaths, _, cx| {
                cx.emit(Ask::Said(kit::lists::import(nr_shown_column(), dropped)));
            }))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        board.update(cx, |board, cx| board.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Route::Event(NrScriptEvent::LookBoard), look);
}

/// The board's column count and vertical scroll, reported by the core once scrolling settles.
/// With no board shown, nothing scrolls.
fn look(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let (offset, shown, height) =
        workspace(window)
            .read(cx)
            .board
            .as_ref()
            .map_or((0.0, 0.0, 0.0), |board| {
                let board = board.read(cx);
                (
                    board.scroll,
                    f32::from(board.frame.size.height),
                    board.height,
                )
            });
    nr_script_looked(f64::from(offset), f64::from(shown), f64::from(height), true);
    Answer::Done
}

fn paint_card(
    window: &mut Window,
    cx: &mut App,
    index: usize,
    id: &str,
    frame: Bounds<Pixels>,
    focus: &NrBoardFocus,
    now: f64,
) {
    rounded(window, frame, color(Token::Card), CARD_RADIUS);
    if focus.drop == index as i64 {
        rounded(window, frame, mixed(Token::Primary, 0.15), CARD_RADIUS);
    }
    paint_card_marks(window, cx, index, id, frame, now);
    if focus.drop == index as i64 {
        stroke(
            window,
            inset(frame, 0.5, 0.5),
            color(Token::Primary),
            11.5,
            1.0,
        );
    }
    if focus.selected == index as i64 {
        // A 1 px accent line at 50% opacity, with a faint dark line inside so it still reads on
        // white media.
        stroke(
            window,
            inset(frame, 1.0, 1.0),
            Hsla::black().alpha(0.3),
            11.0,
            2.0,
        );
        stroke(
            window,
            inset(frame, 0.5, 0.5),
            mixed(Token::Primary, 0.5),
            11.5,
            1.0,
        );
    }
}

/// Paints the core's borrowed marks inside the card. A ready system thumbnail replaces all
/// the fallback marks after it; while it loads, those marks draw the face instead.
fn paint_card_marks(
    window: &mut Window,
    cx: &mut App,
    index: usize,
    id: &str,
    frame: Bounds<Pixels>,
    now: f64,
) {
    let marks = nr_board_card(index, nr_rect(frame));
    let marks: &[NrCardMark] = if marks.count == 0 {
        &[]
    } else {
        // SAFETY: the core lends `count` marks until its next call for a card.
        unsafe { std::slice::from_raw_parts(marks.marks, marks.count) }
    };
    window.with_content_mask(Some(gpui::ContentMask { bounds: frame }), |window| {
        for mark in marks {
            if mark.kind != NrMarkKind::SystemPicture {
                paint_mark(window, cx, mark, index, id, frame, now);
                continue;
            }
            if paint_system_picture(window, mark, id, frame) {
                break;
            }
        }
    });
}

/// A mark's corner radii. A corner that touches the card's corner uses the card's radius (the
/// card clips it); the rest use the mark's own.
fn radii(mark: Bounds<Pixels>, card: Bounds<Pixels>, own: f32) -> Corners<Pixels> {
    let near = |a: Pixels, b: Pixels| (f32::from(a) - f32::from(b)).abs() < 0.75;
    let (left, top) = (
        near(mark.origin.x, card.origin.x),
        near(mark.origin.y, card.origin.y),
    );
    let right = near(
        mark.origin.x + mark.size.width,
        card.origin.x + card.size.width,
    );
    let bottom = near(
        mark.origin.y + mark.size.height,
        card.origin.y + card.size.height,
    );
    let corner = |at: bool| px(if at { CARD_RADIUS.max(own) } else { own });
    Corners {
        top_left: corner(left && top),
        top_right: corner(right && top),
        bottom_left: corner(left && bottom),
        bottom_right: corner(right && bottom),
    }
}

/// Paints the system thumbnail of the card's file, covering the mark's frame from the top so a
/// document shows its first lines. An inner border in the theme colour keeps a white page's
/// edge visible on a light board. False while loading or when the system has no thumbnail.
fn paint_system_picture(
    window: &mut Window,
    mark: &NrCardMark,
    id: &str,
    card_frame: Bounds<Pixels>,
) -> bool {
    let Some(image) = thumbs::system_picture(id, mark.pixels) else {
        return false;
    };
    let frame = rect(&mark.frame);
    cover_from_top(window, &image, frame, radii(frame, card_frame, 0.0));
    stroke(
        window,
        inset(card_frame, 0.5, 0.5),
        color(Token::Border),
        CARD_RADIUS - 0.5,
        1.0,
    );
    true
}

fn mark_color(mark: &NrCardMark) -> Hsla {
    faded(named(&text(&mark.color)), mark.opacity)
}

/// Places a picture or video frame in `frame`, covering it, or fitted whole on the card
/// background when the core says so (`NrCardMark::whole`, for a banner or strip whose card
/// is a different shape).
fn place_picture(
    window: &mut Window,
    image: &Arc<RenderImage>,
    mark: &NrCardMark,
    frame: Bounds<Pixels>,
    card_frame: Bounds<Pixels>,
) {
    if mark.whole {
        kit::draw::contain(window, image, frame, Corners::default());
    } else {
        cover(window, image, frame, radii(frame, card_frame, 0.0));
    }
}

fn paint_mark(
    window: &mut Window,
    cx: &mut App,
    mark: &NrCardMark,
    card: usize,
    id: &str,
    card_frame: Bounds<Pixels>,
    now: f64,
) {
    let frame = rect(&mark.frame);
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
        NrMarkKind::Words | NrMarkKind::PageWords => draw_words(window, cx, &mark.words),
        NrMarkKind::Fill => {
            let corners = radii(frame, card_frame, radius);
            window.paint_quad(gpui::fill(frame, mark_color(mark)).corner_radii(corners));
        }
        NrMarkKind::TodoBox => {
            let (ascent, descent) = metrics(Face::of(&mark.line_font));
            let baseline = browser_baseline(
                mark.line_top as f32,
                mark.line_height as f32,
                ascent,
                descent,
                window.scale_factor(),
            );
            let boxed = kit::core::frame(
                f32::from(frame.origin.x),
                baseline - f32::from(frame.origin.y),
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
        NrMarkKind::Fade => {
            let ground = named(&text(&mark.color));
            let background = linear_gradient(
                180.0,
                linear_color_stop(ground.alpha(0.0), 0.0),
                linear_color_stop(ground, 1.0),
            );
            window.paint_quad(gpui::fill(frame, background));
        }
        NrMarkKind::Oval => oval(window, frame, mark_color(mark)),
        NrMarkKind::Sign => {
            kit::signs::paint(window, cx, &text(&mark.text), frame, mark_color(mark))
        }
        NrMarkKind::FolderArt => paint_folder_art(
            window,
            card,
            frame,
            kit::theme::stated_named(&text(&mark.color)),
            now,
        ),
        NrMarkKind::Picture => {
            if let Some(image) = thumbs::picture(id, mark.pixels, mark.video) {
                place_picture(window, &image, mark, frame, card_frame);
            }
        }
        NrMarkKind::VideoMoment => {
            if let Some(image) = thumbs::video_moment(id, mark.pixels, mark.seconds) {
                place_picture(window, &image, mark, frame, card_frame);
            }
        }
        NrMarkKind::PictureAt => {
            if let Some(image) = thumbs::picture_file(&text(&mark.text), mark.pixels) {
                cover(window, &image, frame, radii(frame, card_frame, 0.0));
            }
        }
        NrMarkKind::Outline => stroke(window, frame, mark_color(mark), radius, mark.stroke as f32),
        NrMarkKind::PictureFile => {
            let ground = named(&text(&mark.color));
            let corners = radii(frame, card_frame, radius);
            window.paint_quad(gpui::fill(frame, ground).corner_radii(corners));
            if let Some(image) = thumbs::picture(&text(&mark.text), mark.pixels, false) {
                cover(window, &image, frame, corners);
            }
        }
        // A terminal's program logo in its own colours, on the tile the earlier marks drew.
        NrMarkKind::Logo => {
            let pixels = f32::from(frame.size.width) * window.scale_factor();
            if let Some(logo) = thumbs::logo(&text(&mark.text), pixels, cx) {
                let _ = window.paint_image(frame, frame, gpui::Corners::default(), logo, 0, false);
            }
        }
        // Painted before the later marks, which it may cover (`paint_card_marks`).
        NrMarkKind::SystemPicture => {}
    }
}

/// Converts a core sRGB colour to the window's colour space (`kit::display`).
fn rgba(color: &NrRgba) -> Hsla {
    kit::display::shown(Rgba {
        r: color.red as f32,
        g: color.green as f32,
        b: color.blue as f32,
        a: color.alpha as f32,
    })
}

/// Paints the folder graphic the core shapes (`nr_board_folder_art`): each shape's contours
/// (rounded rects and polygons) through its transform, filled and optionally stroked.
fn paint_folder_art(
    window: &mut Window,
    card: usize,
    frame: Bounds<Pixels>,
    shade: Rgba,
    now: f64,
) {
    let art = nr_board_folder_art(
        card,
        nr_rect(frame),
        NrRgba {
            red: f64::from(shade.r),
            green: f64::from(shade.g),
            blue: f64::from(shade.b),
            alpha: f64::from(shade.a),
        },
        now,
    );
    if art.count == 0 {
        return;
    }
    // SAFETY: the core lends `count` shapes until its next call.
    let shapes: &[NrArtShape] = unsafe { std::slice::from_raw_parts(art.shapes, art.count) };
    for shape in shapes {
        paint_art_shape(window, shape);
    }
}

/// Paints one folder-graphic shape through its transform, filled, then stroked if it has one.
fn paint_art_shape(window: &mut Window, shape: &NrArtShape) {
    let [a, b, c, d, tx, ty] = shape.transform;
    let place = |x: f64, y: f64| {
        point(
            px((a * x + c * y + tx) as f32),
            px((b * x + d * y + ty) as f32),
        )
    };
    let mut filled = PathBuilder::fill();
    trace_art_shape(&mut filled, shape, &place);
    if let Ok(path) = filled.build() {
        window.paint_path(path, rgba(&shape.fill));
    }
    if shape.stroked {
        let scale = (a * d - b * c).abs().sqrt();
        let mut stroked = PathBuilder::stroke(px((shape.stroke_width * scale) as f32));
        trace_art_shape(&mut stroked, shape, &place);
        if let Ok(path) = stroked.build() {
            window.paint_path(path, rgba(&shape.stroke));
        }
    }
}

/// Traces a shape's contours (rounded rects and polygons) into `builder`, each point through
/// `place`.
fn trace_art_shape(
    builder: &mut PathBuilder,
    shape: &NrArtShape,
    place: &dyn Fn(f64, f64) -> Point<Pixels>,
) {
    if shape.contour_count == 0 {
        return;
    }
    // SAFETY: the shape lends `contour_count` contours.
    let contours = unsafe { std::slice::from_raw_parts(shape.contours, shape.contour_count) };
    for contour in contours {
        if contour.rounded {
            rounded_contour(
                builder,
                &contour.frame,
                contour.radius_x,
                contour.radius_y,
                place,
            );
        } else if contour.point_count > 0 {
            // SAFETY: the contour lends `point_count` points.
            let points = unsafe { std::slice::from_raw_parts(contour.points, contour.point_count) };
            builder.move_to(place(points[0].x, points[0].y));
            for at in &points[1..] {
                builder.line_to(place(at.x, at.y));
            }
            builder.close();
        }
    }
}

/// A rounded rect traced as Core Graphics' `addRoundedRect` does, with quarter-ellipse corners.
fn rounded_contour(
    builder: &mut PathBuilder,
    frame: &NrRect,
    rx: f64,
    ry: f64,
    place: &dyn Fn(f64, f64) -> Point<Pixels>,
) {
    let (x0, y0, x1, y1) = (
        frame.x,
        frame.y,
        frame.x + frame.width,
        frame.y + frame.height,
    );
    let (rx, ry) = (rx.min(frame.width / 2.0), ry.min(frame.height / 2.0));
    // Cubic control-point distance for a quarter ellipse.
    const K: f64 = 0.552_284_75;
    builder.move_to(place(x0 + rx, y0));
    builder.line_to(place(x1 - rx, y0));
    builder.cubic_bezier_to(
        place(x1, y0 + ry),
        place(x1 - rx + rx * K, y0),
        place(x1, y0 + ry - ry * K),
    );
    builder.line_to(place(x1, y1 - ry));
    builder.cubic_bezier_to(
        place(x1 - rx, y1),
        place(x1, y1 - ry + ry * K),
        place(x1 - rx + rx * K, y1),
    );
    builder.line_to(place(x0 + rx, y1));
    builder.cubic_bezier_to(
        place(x0, y1 - ry),
        place(x0 + rx - rx * K, y1),
        place(x0, y1 - ry + ry * K),
    );
    builder.line_to(place(x0, y0 + ry));
    builder.cubic_bezier_to(
        place(x0 + rx, y0),
        place(x0, y0 + ry - ry * K),
        place(x0 + rx - rx * K, y0),
    );
    builder.close();
}
