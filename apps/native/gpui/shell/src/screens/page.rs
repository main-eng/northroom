//! The page editor. The core lays the page out (`page::layout`) on the panel background in a
//! scroller, and its marks draw like a card's. Pointer and key feedback draws on top
//! (`nr_page_overlay`), and the block being edited is a text field over its text (`field`).
//! Presses, gutter hover, keys, typed text and input-method composition go to the core's page
//! editor (`ffi::page_*`), and its `Effect`s run in order. The core also owns the `/` menu
//! (`slash`), the block handle menu and the table cell menu.
//!
//! `page_players` owns the page's `<video>` and `<audio>` players, their controls and the
//! preview they wake. The page lays them out from the core's layout callback and puts their
//! view above its blocks.

mod field;
mod marks;
mod slash;

use super::page_players::{KeyHolder, PagePlayers, PageState, key_holder};
use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::probe::Probe;
use crate::script::{Answer as Done, Pointer, Route, Step, Table};
use field::{Field, Slot};
use gpui::{
    App, AppContext, Bounds, Context, CursorStyle, DispatchPhase, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Hitbox, HitboxBehavior, IntoElement, KeyBinding,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, Role, ScrollDelta, ScrollEvent, Styled, UTF16Selection, WeakEntity, Window,
    actions, canvas, point, prelude::*, px,
};
use kit::controls::edit::{
    from_utf16, next_grapheme, next_word, previous_grapheme, previous_word, range_from_utf16,
    range_to_utf16, to_utf16, word_at,
};
use kit::controls::input;
use kit::core::{c, contains, inset, rect};
use kit::lists::scroll::Scroller;
use kit::motion::now;
use kit::nr::*;
use kit::platform::{read_clipboard, write_clipboard};
use kit::theme::{Token, color};
use northroom_core::page::editor::{Answer, Effect, Typing};
use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

actions!(
    page_field,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteBackward,
        DeleteForward,
        DeleteWordBackward,
        Newline,
        Tab,
        Backtab,
        Cancel,
    ]
);

/// The key context for a page field, with `NSTextView`'s key bindings. Keys with ⌘ or Control
/// go to the page first (`page_field_shortcut`).
const CONTEXT: &str = "PageField";

pub fn bindings() -> Vec<KeyBinding> {
    let context = Some(CONTEXT);
    vec![
        KeyBinding::new("left", MoveLeft, context),
        KeyBinding::new("right", MoveRight, context),
        KeyBinding::new("up", MoveUp, context),
        KeyBinding::new("down", MoveDown, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("shift-up", SelectUp, context),
        KeyBinding::new("shift-down", SelectDown, context),
        KeyBinding::new("alt-left", WordLeft, context),
        KeyBinding::new("alt-right", WordRight, context),
        KeyBinding::new("alt-shift-left", SelectWordLeft, context),
        KeyBinding::new("alt-shift-right", SelectWordRight, context),
        KeyBinding::new("backspace", DeleteBackward, context),
        KeyBinding::new("shift-backspace", DeleteBackward, context),
        KeyBinding::new("alt-backspace", DeleteWordBackward, context),
        KeyBinding::new("delete", DeleteForward, context),
        KeyBinding::new("enter", Newline, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("tab", Tab, context),
        KeyBinding::new("shift-tab", Backtab, context),
        KeyBinding::new("escape", Cancel, context),
    ]
}

/// The caret's on and off time, matching `NSTextView`.
const BLINK: Duration = Duration::from_millis(530);

thread_local! {
    /// The window's page view, which a chosen menu entry reaches (`menu_chosen`).
    static LIVE: RefCell<Option<WeakEntity<Page>>> = const { RefCell::new(None) };
    /// The players placed during the current layout (`place_player`).
    static PLACING: RefCell<Option<Placing>> = const { RefCell::new(None) };
}

/// The result of running an editor answer.
#[derive(Default)]
struct Outcome {
    taken: bool,
    passed: bool,
    drag: bool,
    forward: bool,
}

// MARK: Players

/// The page's players and the app during a core layout. The layout callback (`place_player`)
/// lays out each player block through them.
struct Placing {
    players: Entity<PagePlayers>,
    app: *mut App,
}

/// Places player block `block` for the core's layout (`NrPlacePlayer`), with its top at `top`
/// in a column `column` wide starting at `side` (`PagePlayers::lay`).
extern "C" fn place_player(block: usize, top: f64, side: f64, column: f64) -> NrPagePlayerPlaced {
    PLACING.with(|placing| {
        let placing = placing.borrow();
        let placing = placing
            .as_ref()
            .expect("players are placed only while a page is laid out");
        // SAFETY: `app` is the app the current layout was requested with, and nothing else
        // holds it until `page::layout` returns (`Page::lay_out`).
        let app = unsafe { &mut *placing.app };
        placing
            .players
            .update(app, |players, cx| players.lay(block, top, side, column, cx))
    })
}

// MARK: Layout

/// The open page as the core last laid it out for this view.
struct Laid {
    /// The core's layout. Its marks are borrowed until the view's next layout.
    layout: NrPageLayout,
    slots: Vec<Slot>,
    blocks: Vec<Bounds<Pixels>>,
    side: f32,
    height: f32,
    width: f32,
    /// The open field's text at layout time, which the edited block held.
    typed: Option<String>,
}

impl Laid {
    fn marks(&self) -> Vec<(&NrCardMark, &NrPageMarkShown)> {
        let count = self.layout.mark_count;
        if count == 0 {
            return Vec::new();
        }
        // SAFETY: the core lends `mark_count` marks and as many `shown` until the view's next
        // layout.
        let (marks, shown) = unsafe {
            (
                std::slice::from_raw_parts(self.layout.marks, count),
                std::slice::from_raw_parts(self.layout.shown, count),
            )
        };
        marks.iter().zip(shown).collect()
    }
}

/// A press on a block handle, tracked until release.
struct Following {
    start: Point<Pixels>,
    dragging: bool,
    shift: bool,
}

pub struct Page {
    /// The page's own focus, for selected blocks and media blocks.
    focus: FocusHandle,
    /// The text field's focus.
    field_focus: FocusHandle,
    /// Where the window placed the page.
    frame: Bounds<Pixels>,
    scroller: Scroller,
    laid: Option<Laid>,
    /// The layout is stale and is rebuilt before the next draw or hit test.
    stale: bool,
    players: Entity<PagePlayers>,
    /// What had the keys at the last draw, for the players' report.
    keys: Rc<RefCell<Option<KeyHolder>>>,
    field: Option<Field>,
    /// The open `/` menu and the block it hangs under.
    slash: Option<(i64, northroom_core::menus::slash::Layout)>,
    following: Option<Following>,
    /// A press in the field is held, so a drag selects.
    selecting: bool,
    /// Whether the pointer is over the page.
    hovering: bool,
    caret_shown: bool,
    blink_epoch: usize,
    tracing: bool,
}

impl Page {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        nr_page_load();
        // A new page view starts with nothing edited, selected or hovered.
        nr_page_editor_reset();
        let field_focus = cx.focus_handle();
        cx.on_blur(&field_focus, window, |page, window, cx| {
            // The field lost focus, so editing ends and keeps the text. The window moving behind
            // another doesn't count (GPUI reports it as a blur; AppKit's field keeps editing).
            if page.field_focus.is_focused(window) {
                return;
            }
            if let Some(field) = &page.field {
                let said = page_end_typing(&field.typing());
                page.run(said, window, cx);
            }
        })
        .detach();
        LIVE.with(|live| *live.borrow_mut() = Some(cx.entity().downgrade()));
        // The core keeps each page view's last layout (`page::layout`) until the view drops.
        let view = cx.entity_id().as_u64() as usize;
        cx.on_release(move |_, _| nr_page_forget(view)).detach();
        let players = cx.new(PagePlayers::new);
        let keys = Rc::new(RefCell::new(None));
        let reported = keys.clone();
        let focus = cx.focus_handle();
        let (page_keys, field_keys) = (focus.clone(), field_focus.clone());
        players.update(cx, |players, _| {
            players.set_state(Rc::new(move |window, _| {
                let state = nr_page_editor();
                // With a window, read focus now; without one, use the last draw.
                let keys = match window {
                    Some((window, controls)) => {
                        key_holder(window, &field_keys, &page_keys, controls)
                    }
                    None => *reported.borrow(),
                };
                PageState {
                    chosen: (state.chosen_first >= 0)
                        .then_some((state.chosen_first, state.chosen_last)),
                    // -1: no block is being edited.
                    editing: (state.block >= 0).then_some(state.block),
                    keys,
                }
            }))
        });
        Self {
            focus,
            field_focus,
            frame: Bounds::default(),
            scroller: Scroller::default(),
            laid: None,
            stale: true,
            players,
            keys,
            field: None,
            slash: None,
            following: None,
            selecting: false,
            hovering: false,
            caret_shown: true,
            blink_epoch: 0,
            tracing: nr_page_tracing(),
        }
    }

    /// The key the core stores this view's layout under.
    fn key(&self, cx: &Context<Self>) -> usize {
        cx.entity_id().as_u64() as usize
    }

    /// The page's top-left in the window at the current scroll.
    fn origin(&self) -> Point<Pixels> {
        point(
            self.frame.origin.x,
            self.frame.origin.y - px(self.scroller.offset()),
        )
    }

    /// Converts a window point to page coordinates.
    fn in_page(&self, at: Point<Pixels>) -> Point<Pixels> {
        at - self.origin()
    }

    // MARK: Layout

    /// Lays the page out again (`page::layout`) when it is stale: requested, the width changed,
    /// or the open field's text differs from the last layout.
    fn lay_out(&mut self, window: &Window, cx: &mut Context<Self>) {
        let (width, viewport) = (
            f32::from(self.frame.size.width),
            f32::from(self.frame.size.height),
        );
        let typed = self.field.as_ref().map(|field| field.text.as_str());
        if width <= 0.0
            || (!self.stale
                && self
                    .laid
                    .as_ref()
                    .is_some_and(|laid| laid.width == width && laid.typed.as_deref() == typed))
        {
            return;
        }
        let typed = typed.map(str::to_owned);
        self.stale = false;
        let key = self.key(cx);
        self.players.update(cx, |players, _| players.begin());
        let app: &mut App = cx;
        PLACING.with(|placing| {
            *placing.borrow_mut() = Some(Placing {
                players: self.players.clone(),
                app,
            })
        });
        let layout = page_layout(
            key,
            f64::from(width),
            f64::from(viewport),
            f64::from(window.viewport_size().width),
            place_player,
            typed.as_deref(),
        );
        PLACING.with(|placing| *placing.borrow_mut() = None);
        self.players.update(cx, |players, cx| players.end(cx));
        let slots = if layout.slot_count == 0 {
            Vec::new()
        } else {
            // SAFETY: the core lends `slot_count` slots until the view's next layout.
            unsafe { std::slice::from_raw_parts(layout.slots, layout.slot_count) }
                .iter()
                .map(Slot::of)
                .collect()
        };
        let blocks = if layout.block_count == 0 {
            Vec::new()
        } else {
            // SAFETY: as for the slots.
            unsafe { std::slice::from_raw_parts(layout.blocks, layout.block_count) }
                .iter()
                .map(|block| rect(&block.frame))
                .collect()
        };
        let (side, height) = (layout.side as f32, layout.height as f32);
        self.scroller.set_extent(viewport, height.max(viewport));
        self.laid = Some(Laid {
            layout,
            slots,
            blocks,
            side,
            height,
            width,
            typed,
        });
        // An open field moves with its slot to the new layout.
        if let (Some(field), Some(laid)) = (&mut self.field, &self.laid)
            && let Some(slot) = laid.slots.iter().find(|slot| {
                slot.block == field.slot.block
                    && slot.row == field.slot.row
                    && slot.column == field.slot.column
                    && slot.caption == field.slot.caption
            })
        {
            field.slot = slot.clone();
        }
    }

    // MARK: Editor answers

    /// The field state sent to the core with every call: its text and selection in UTF-16
    /// offsets; empty while nothing is being edited.
    fn typing(&self) -> Typing<'_> {
        self.field.as_ref().map_or(Typing::NONE, Field::typing)
    }

    /// Runs the editor's answer in order.
    fn run(&mut self, answer: Answer, window: &mut Window, cx: &mut Context<Self>) -> Outcome {
        let mut outcome = Outcome {
            taken: answer.taken,
            ..Outcome::default()
        };
        for effect in answer.effects {
            match effect {
                Effect::EndEdit => self.end_edit(window, cx),
                Effect::Relayout => {
                    self.stale = true;
                    self.lay_out(window, cx);
                }
                Effect::Begin {
                    block,
                    row,
                    column,
                    caption,
                    text,
                    caret,
                } => self.begin(block, row, column, caption, text, caret, window, cx),
                Effect::Apply { text, caret } => {
                    self.set_field_text(text, caret);
                    self.trace("edit");
                }
                Effect::Retype { text, caret } => {
                    self.set_field_text(text, caret);
                    self.selection_changed(window, cx);
                }
                Effect::Place { start, end } => self.select_field_units(start, end),
                Effect::Select { start, end } => {
                    self.select_field_units(start, end);
                    self.selection_changed(window, cx);
                }
                Effect::Insert { text, start, end } => {
                    self.insert_over_units(&text, start..end, window, cx);
                }
                Effect::Restyle => {
                    if let Some(field) = &mut self.field {
                        field.restyle();
                    }
                }
                Effect::Trace { step } => self.trace(step),
                Effect::TakeKeys => window.focus(&self.focus, cx),
                Effect::Beep => window.play_system_bell(),
                Effect::Copy { text } => write_clipboard(&text, cx),
                Effect::Paste => self.paste_blocks(window, cx),
                Effect::Vertical { up } => outcome.taken = self.move_vertically(up, window, cx),
                Effect::BlockMenu { block } => self.block_menu(block, cx),
                Effect::Slash => self.show_slash(),
                Effect::Pass => outcome.passed = true,
                Effect::Drag => outcome.drag = true,
                Effect::Forward => outcome.forward = true,
            }
        }
        self.blink_from_now(cx);
        cx.notify();
        outcome
    }

    /// Closes the field and releases focus if it had it.
    fn end_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.take().is_some() && self.field_focus.is_focused(window) {
            window.blur(cx);
        }
    }

    /// Sets the open field's text with the caret at `caret` (UTF-16).
    fn set_field_text(&mut self, text: String, caret: usize) {
        if let Some(field) = &mut self.field {
            field.set_text(text, caret);
        }
    }

    /// Selects `start..end` (UTF-16) in the open field.
    fn select_field_units(&mut self, start: usize, end: usize) {
        if let Some(field) = &mut self.field {
            field.select_units(start, end);
        }
    }

    /// Replaces `units` (UTF-16) of the open field with `text`.
    fn insert_over_units(
        &mut self,
        text: &str,
        units: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = &self.field {
            let range = range_from_utf16(&field.text, &units);
            self.type_over(Some(range), text, window, cx);
        }
    }

    /// Pastes clipboard Markdown as page blocks, split by the core.
    fn paste_blocks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(markdown) = read_clipboard(cx) {
            let said = page_paste_blocks(&markdown);
            self.run(said, window, cx);
        }
    }

    /// Opens a field over a slot's text, holding `text` as written (inline marks spelled out)
    /// with the caret at `caret`. It uses the slot's own type so the line doesn't shift.
    #[allow(clippy::too_many_arguments)]
    fn begin(
        &mut self,
        block: i64,
        row: i64,
        column: i64,
        caption: bool,
        text: String,
        caret: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.lay_out(window, cx);
        let Some(slot) = self.laid.as_ref().and_then(|laid| {
            laid.slots
                .iter()
                .find(|slot| {
                    slot.block == block
                        && slot.row == row
                        && slot.column == column
                        && slot.caption == caption
                })
                .cloned()
        }) else {
            return;
        };
        let formatted = nr_page_editor().formatted;
        self.field = Some(Field::new(slot, text, caret, formatted));
        window.focus(&self.field_focus, cx);
        self.trace("begin");
    }

    /// The field's selection changed; the core says where it may rest (`page_selection`).
    fn selection_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.field.is_none() {
            return;
        }
        let said = page_selection(&self.typing());
        self.run(said, window, cx);
        window.notify_text_selection_changed(&self.field_focus);
    }

    /// Moves the caret a line up or down, with the core deciding where it rests
    /// (`page_vertical`): beside an image, never inside it.
    fn move_vertically(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(field) = &mut self.field else {
            return false;
        };
        let from = to_utf16(&field.text, field.caret());
        field.move_vertically(up, false);
        let said = page_vertical(from, up, &self.typing());
        self.run(said, window, cx).taken
    }

    /// Opens the block handle menu (`menus::ffi::menu_page_block`) 4 pt below the block's row,
    /// aligned to its left edge.
    fn block_menu(&mut self, block: usize, cx: &mut Context<Self>) {
        let Some(laid) = &self.laid else { return };
        let Some(frame) = laid.blocks.get(block) else {
            return;
        };
        let origin = self.origin();
        let at = point(
            origin.x + px(laid.side),
            origin.y + frame.origin.y + frame.size.height + px(4.0),
        );
        cx.emit(Ask::Menu {
            which: NrMenu::PageBlock,
            again: Rc::new(move |_| menu_page_block(block)),
            at,
            below: true,
        });
    }

    /// What in the page has the keys: the field, the page or a player's controls.
    fn has_keys(&self, window: &Window, cx: &App) -> Option<KeyHolder> {
        key_holder(window, &self.field_focus, &self.focus, || {
            self.players.read(cx).controls_have_keys(window, cx)
        })
    }

    /// Reads the `/` menu from the core (`page_slash`).
    fn show_slash(&mut self) {
        self.slash = slash::read();
    }

    /// Where the `/` menu hangs, in window points: left-aligned with the block's text, 4 pt below
    /// the block's box, which ends 2 pt below the text.
    fn slash_frame(&self) -> Option<Bounds<Pixels>> {
        let (block, menu) = self.slash.as_ref()?;
        let slot = self
            .laid
            .as_ref()?
            .slots
            .iter()
            .find(|slot| slot.block == *block)?;
        let origin = self.origin();
        Some(Bounds {
            origin: point(
                origin.x + slot.frame.origin.x - px(2.0),
                origin.y + slot.frame.origin.y + slot.frame.size.height + px(6.0),
            ),
            size: gpui::size(px(menu.width as f32), px(menu.height as f32)),
        })
    }

    fn choose_slash(&mut self, command: usize, window: &mut Window, cx: &mut Context<Self>) {
        let said = page_slash_choose(command, &self.typing());
        self.run(said, window, cx);
    }

    /// For a check (`NR_PAGE_TRACE`), writes the edited line's state as one JSON line on stderr:
    /// the block, its written text, the selection, the drawn text (each picture as
    /// `[picture]`), its images and the caret position.
    fn trace(&mut self, step: &str) {
        if !self.tracing {
            return;
        }
        let state = nr_page_editor();
        let mut said = serde_json::json!({
            "step": step,
            "editing": state.block,
            "focusedMedia": state.media,
        });
        if let Some(field) = &mut self.field {
            let selection = range_to_utf16(&field.text, &field.selected);
            let written = field.text.clone();
            let (drawn, images) = {
                let laid = field.laid();
                (
                    laid.drawn(),
                    laid.images
                        .iter()
                        .map(|image| {
                            let units = range_to_utf16(&written, image);
                            vec![units.start, units.end]
                        })
                        .collect::<Vec<_>>(),
                )
            };
            let (caret_line, lines) = field.trace_lines();
            said["text"] = written.into();
            said["selection"] = vec![selection.start, selection.end - selection.start].into();
            said["drawn"] = drawn.into();
            said["images"] = images.into();
            said["caretLine"] = caret_line.into();
            said["lines"] = lines.into();
        }
        eprintln!("page\t{said}");
    }

    // MARK: Caret

    /// Shows the caret and restarts its blink.
    fn blink_from_now(&mut self, cx: &mut Context<Self>) {
        self.caret_shown = true;
        self.blink_epoch += 1;
        if self.field.is_none() {
            return;
        }
        let epoch = self.blink_epoch;
        cx.spawn(async move |page, cx| {
            loop {
                cx.background_executor().timer(BLINK).await;
                let going = page.update(cx, |page, cx| page.blink(epoch, cx));
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Toggles the caret for the blink started at `epoch`. Returns whether that blink continues
    /// (no newer one started and a field is still open).
    fn blink(&mut self, epoch: usize, cx: &mut Context<Self>) -> bool {
        if self.blink_epoch != epoch || self.field.is_none() {
            return false;
        }
        self.caret_shown = !self.caret_shown;
        cx.notify();
        true
    }

    /// Scrolls just enough to show the caret's line.
    fn reveal_caret(&mut self) {
        let Some(field) = &mut self.field else { return };
        let (_, top, height) = field.caret_line();
        let at = f32::from(field.slot.frame.origin.y) + top;
        self.scroller.reveal(at, at + height);
    }

    // MARK: Typing

    /// Sends typed, pasted or deleted text through the core's rules (`page_typed`). The core
    /// makes the change unless the field keeps it.
    fn type_over(
        &mut self,
        range: Option<Range<usize>>,
        replacement: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &self.field else { return };
        let range = range
            .or(field.marked.clone())
            .unwrap_or(field.selected.clone());
        let marked = field.marked.is_some();
        let over = range_to_utf16(&field.text, &range);
        let said = page_typed(over.start, over.end, replacement, marked, &self.typing());
        if self.run(said, window, cx).taken {
            window.notify_text_input_changed(&self.field_focus);
            return;
        }
        let Some(field) = &mut self.field else { return };
        field.text.replace_range(range.clone(), replacement);
        let caret = range.start + replacement.len();
        field.move_to(caret);
        field.marked = None;
        field.goal = None;
        let said = page_changed(&self.typing());
        self.run(said, window, cx);
        window.notify_text_input_changed(&self.field_focus);
    }

    /// Runs a field command (`page_text_command`); true when the core handled it.
    fn command(
        &mut self,
        key: NrKey,
        held: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let press = NrKeyPress {
            key,
            character: 0,
            letter: 0,
            held,
        };
        let said = page_text_command(press, &self.typing());
        self.run(said, window, cx).taken
    }

    /// Moves the caret, or the selection's active end, to `to` in the field's text.
    fn caret_to(
        &mut self,
        to: impl FnOnce(&mut Field) -> usize,
        extend: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &mut self.field else { return };
        let index = to(field);
        if extend {
            field.select_to(index);
        } else {
            field.move_to(index);
        }
        field.goal = None;
        self.selection_changed(window, cx);
        self.reveal_caret();
        cx.notify();
    }

    fn move_left(&mut self, _: &MoveLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Left, 0, window, cx) {
            self.caret_to(
                |field| {
                    if field.selected.is_empty() {
                        previous_grapheme(&field.text, field.caret())
                    } else {
                        field.selected.start
                    }
                },
                false,
                window,
                cx,
            );
        }
    }

    fn move_right(&mut self, _: &MoveRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Right, 0, window, cx) {
            self.caret_to(
                |field| {
                    if field.selected.is_empty() {
                        next_grapheme(&field.text, field.caret())
                    } else {
                        field.selected.end
                    }
                },
                false,
                window,
                cx,
            );
        }
    }

    fn vertical(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let goal = self.field.as_ref().and_then(|field| field.goal);
        let key = if up { NrKey::Up } else { NrKey::Down };
        if self.command(key, 0, window, cx) {
            // The core handled the key: an image's line, or the block above or below.
            if let Some(field) = &mut self.field {
                field.goal = goal.or(field.goal);
            }
            self.reveal_caret();
            return;
        }
        if let Some(field) = &mut self.field {
            field.move_vertically(up, false);
        }
        self.selection_changed(window, cx);
        self.reveal_caret();
    }

    fn move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, window, cx);
    }

    fn move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_to(|field| field.step(field.caret(), false), true, window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_to(|field| field.step(field.caret(), true), true, window, cx);
    }

    fn select_vertically(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &mut self.field {
            field.move_vertically(up, true);
        }
        self.selection_changed(window, cx);
        self.reveal_caret();
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        self.select_vertically(true, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        self.select_vertically(false, window, cx);
    }

    fn word_left(&mut self, _: &WordLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_to(
            |field| previous_word(&field.text, field.caret()),
            false,
            window,
            cx,
        );
    }

    fn word_right(&mut self, _: &WordRight, window: &mut Window, cx: &mut Context<Self>) {
        self.caret_to(
            |field| next_word(&field.text, field.caret()),
            false,
            window,
            cx,
        );
    }

    fn select_word_left(
        &mut self,
        _: &SelectWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.caret_to(
            |field| previous_word(&field.text, field.caret()),
            true,
            window,
            cx,
        );
    }

    fn select_word_right(
        &mut self,
        _: &SelectWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.caret_to(
            |field| next_word(&field.text, field.caret()),
            true,
            window,
            cx,
        );
    }

    /// Deletes the selection, or with none, the text `before` the caret reaches, through the
    /// core's rules.
    fn delete(
        &mut self,
        before: impl FnOnce(&Field) -> Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &self.field else { return };
        let range = if field.selected.is_empty() {
            before(field)
        } else {
            field.selected.clone()
        };
        self.type_over(Some(range), "", window, cx);
        self.reveal_caret();
    }

    fn delete_backward(&mut self, _: &DeleteBackward, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Backspace, 0, window, cx) {
            self.delete(
                |field| previous_grapheme(&field.text, field.caret())..field.caret(),
                window,
                cx,
            );
        }
    }

    fn delete_word_backward(
        &mut self,
        _: &DeleteWordBackward,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.delete(
            |field| previous_word(&field.text, field.caret())..field.caret(),
            window,
            cx,
        );
    }

    fn delete_forward(&mut self, _: &DeleteForward, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Delete, 0, window, cx) {
            self.delete(
                |field| field.caret()..next_grapheme(&field.text, field.caret()),
                window,
                cx,
            );
        }
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Return, 0, window, cx) {
            self.type_over(None, "\n", window, cx);
        }
        self.reveal_caret();
    }

    fn tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command(NrKey::Tab, 0, window, cx) {
            self.type_over(None, "\t", window, cx);
        }
    }

    fn backtab(&mut self, _: &Backtab, window: &mut Window, cx: &mut Context<Self>) {
        self.command(NrKey::Tab, NR_HELD_SHIFT, window, cx);
    }

    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.command(NrKey::Escape, 0, window, cx);
    }

    /// Handles a ⌘ or Control key while the field has focus: page shortcuts first
    /// (`page_field_shortcut`), then the field's (select all, clipboard, line and field ends),
    /// then the menu bar.
    fn field_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.field_shortcut(&event.keystroke, window, cx) {
            cx.stop_propagation();
        }
    }

    /// Handles a ⌘ or Control key while a field is open: the page's shortcut first
    /// (`page_field_shortcut`), else the field's own. Returns whether it was handled.
    fn field_shortcut(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let press = kit::keys::press(keystroke);
        if press.held & (NR_HELD_COMMAND | NR_HELD_CONTROL) == 0 || self.field.is_none() {
            return false;
        }
        let said = page_field_shortcut(press, &self.typing());
        if self.run(said, window, cx).taken {
            return true;
        }
        let modifiers = &keystroke.modifiers;
        if !modifiers.platform || modifiers.control || modifiers.alt {
            return false;
        }
        let shift = modifiers.shift;
        match (keystroke.key.as_str(), shift) {
            ("a", false) => self.select_all_words(window, cx),
            ("c", false) => return self.copy_selection(cx),
            ("x", false) => {
                if !self.copy_selection(cx) {
                    return false;
                }
                self.type_over(None, "", window, cx);
            }
            ("v", false) => self.paste_words(window, cx),
            ("left", extend) => self.caret_to(|field| line_start(field), extend, window, cx),
            ("right", extend) => self.caret_to(|field| line_end(field), extend, window, cx),
            ("up", extend) => self.caret_to(|_| 0, extend, window, cx),
            ("down", extend) => self.caret_to(|field| field.text.len(), extend, window, cx),
            _ => return false,
        }
        true
    }

    /// Selects all text in the open field.
    fn select_all_words(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &mut self.field {
            field.move_to(0);
            field.select_to(field.text.len());
        }
        self.selection_changed(window, cx);
    }

    /// Copies the open field's selection; returns whether anything was selected.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(field) = self
            .field
            .as_ref()
            .filter(|field| !field.selected.is_empty())
        else {
            return false;
        };
        write_clipboard(&field.text[field.selected.clone()], cx);
        true
    }

    /// Pastes clipboard text over the selection and reveals the caret after it.
    fn paste_words(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(pasted) = read_clipboard(cx) {
            self.type_over(None, &pasted, window, cx);
            self.reveal_caret();
        }
    }

    // MARK: Edit menu text commands, handled as `NSTextView` does. Cut, Copy, Paste and Select
    // All act as their keys, Paste and Match Style acts as Paste (text is plain), Delete
    // removes the selection, and Start and Stop Speaking.

    /// Runs a menu command on the field as its shortcut key.
    fn as_key(&mut self, keys: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = gpui::Keystroke::parse(keys).expect("a menu's key is a keystroke");
        self.field_shortcut(&keystroke, window, cx);
    }

    fn menu_cut(&mut self, _: &input::Cut, window: &mut Window, cx: &mut Context<Self>) {
        self.as_key("cmd-x", window, cx);
    }

    fn menu_copy(&mut self, _: &input::Copy, window: &mut Window, cx: &mut Context<Self>) {
        self.as_key("cmd-c", window, cx);
    }

    fn menu_paste(&mut self, _: &input::Paste, window: &mut Window, cx: &mut Context<Self>) {
        self.as_key("cmd-v", window, cx);
    }

    fn menu_paste_plain(
        &mut self,
        _: &input::PasteAndMatchStyle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.as_key("cmd-v", window, cx);
    }

    fn menu_select_all(
        &mut self,
        _: &input::SelectAll,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.as_key("cmd-a", window, cx);
    }

    /// Delete removes the selection without copying it. It is unavailable with no selection.
    fn menu_delete(
        &mut self,
        _: &input::DeleteSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.type_over(None, "", window, cx);
    }

    /// Reads the selection aloud, or the whole field when nothing is selected.
    fn menu_speak(&mut self, _: &input::StartSpeaking, _: &mut Window, _: &mut Context<Self>) {
        if let Some(field) = &self.field {
            let words = if field.selected.is_empty() {
                &field.text[..]
            } else {
                &field.text[field.selected.clone()]
            };
            kit::platform::start_speaking(words);
        }
    }

    fn menu_stop_speaking(
        &mut self,
        _: &input::StopSpeaking,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        kit::platform::stop_speaking();
    }

    // MARK: Page keys

    /// Handles a key while the page has focus: shortcuts first (`page_shortcut`), else
    /// `page_key`. Keys the page passes go on to the window.
    fn page_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus.is_focused(window) {
            return;
        }
        let press = kit::keys::press(&event.keystroke);
        if press.held & (NR_HELD_COMMAND | NR_HELD_CONTROL) != 0 {
            if self.run(page_shortcut(press), window, cx).taken {
                cx.stop_propagation();
            }
            return;
        }
        let outcome = self.run(page_key(press), window, cx);
        if !outcome.passed {
            cx.stop_propagation();
        }
    }

    // MARK: Pointer

    /// What a press at page point `at` hits, per the core's hit test (`nr_page_hit`), with its
    /// slot named as a field.
    fn press_at(&self, at: Point<Pixels>, cx: &Context<Self>) -> NrPagePress {
        let hit = nr_page_hit(self.key(cx), f64::from(at.x), f64::from(at.y));
        let mut press = NrPagePress {
            row: hit.row,
            button: hit.button,
            block: hit.block,
            media: hit.media,
            todo: hit.todo,
            slot: 0,
            slot_block: -1,
            slot_row: -1,
            slot_column: -1,
            below: hit.below,
        };
        if let Some(slot) = usize::try_from(hit.slot)
            .ok()
            .and_then(|slot| self.laid.as_ref()?.slots.get(slot))
        {
            press.slot = slot.field();
            press.slot_block = slot.block;
            press.slot_row = slot.row;
            press.slot_column = slot.column;
        }
        press
    }

    /// Handles a press on the page as the core says (`page_press`). A press on text goes to the
    /// field, which places the caret, selects the word on a double click and tracks drags.
    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.lay_out(window, cx);
        if self.scroller.press(event.position, self.frame, now()) {
            cx.notify();
            return;
        }
        if let Some(menu) = self.slash_frame()
            && contains(&menu, event.position)
        {
            // The press keeps the caret in the line; the click selects the block.
            return;
        }
        let at = self.in_page(event.position);
        let press = self.press_at(at, cx);
        let said = page_press(press, event.modifiers.shift, &self.typing());
        let outcome = self.run(said, window, cx);
        // A press that opened no field gives the page focus, as AppKit makes the pressed view
        // first responder.
        if self.field.is_none() {
            window.focus(&self.focus, cx);
        }
        if outcome.drag {
            self.following = Some(Following {
                start: event.position,
                dragging: false,
                shift: event.modifiers.shift,
            });
        }
        if outcome.forward {
            self.press_field(event, at, window, cx);
        }
    }

    /// Handles a press the editor forwarded to the open field at page point `at`, if it lands on
    /// the field. Places the caret, or selects the word, line or image on a double or triple
    /// click.
    fn press_field(
        &mut self,
        event: &MouseDownEvent,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &mut self.field else { return };
        if !contains(&inset(field.frame(), -2.0, -2.0), at) {
            return;
        }
        let inside = at - field.slot.frame.origin;
        let pressed = field
            .formatted
            .then(|| field.laid().image_at(f32::from(inside.y)))
            .flatten();
        let index = field.laid().index_at(inside);
        if event.click_count >= 2 {
            // A double click selects the word (a triple, the line); on an image, the image.
            let image = pressed.as_ref().map(|(image, _)| image);
            field.selected = clicked_selection(&field.text, image, index, event.click_count);
            field.reversed = false;
            self.selection_changed(window, cx);
        } else {
            if event.modifiers.shift {
                field.select_to(index);
            } else {
                field.move_to(index);
            }
            field.goal = None;
            self.selecting = true;
            self.selection_changed(window, cx);
            if let Some((_, on)) = pressed {
                self.caret_after_image(on, window, cx);
            }
        }
        self.trace("press");
        cx.notify();
    }

    /// A press on an image puts the caret after it on the line below and leaves the image
    /// unchanged; the core says where (`nr_inline_press`). `on` is the image's placeholder
    /// character.
    fn caret_after_image(&mut self, on: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(field) = self
            .field
            .as_mut()
            .filter(|field| field.selected.is_empty())
        else {
            return;
        };
        let written = c(&field.text);
        let on = to_utf16(&field.text, on);
        // SAFETY: a NUL-terminated string.
        let caret = unsafe { nr_inline_press(written.as_ptr(), on) };
        if caret >= 0 {
            let caret = from_utf16(&field.text, caret as usize);
            field.move_to(caret);
            self.selection_changed(window, cx);
        }
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.scroller.is_held() || self.scroller.pointer(event.position, self.frame, now()) {
            cx.notify();
        }
        let at = self.in_page(event.position);
        if let Some(following) = &self.following {
            let start = following.start;
            self.follow_drag(event.position - start, at, window, cx);
            return;
        }
        if self.selecting && event.pressed_button == Some(MouseButton::Left) {
            self.select_by_drag(at, window, cx);
            return;
        }
        if self.light_slash_row(event.position, cx) {
            return;
        }
        self.hover(event.position, over, cx);
    }

    /// Handles a drag the editor tracks (a block handle), `moved` from its start with the
    /// pointer at page point `at`. The core decides what it does; once taken it is dragging.
    fn follow_drag(
        &mut self,
        moved: Point<Pixels>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let scale = f64::from(window.scale_factor());
        let (dx, dy) = (f64::from(moved.x), f64::from(moved.y));
        let said = page_handle_move(self.key(cx), f64::from(at.y), dx, dy, scale, &self.typing());
        if self.run(said, window, cx).taken
            && let Some(following) = &mut self.following
        {
            following.dragging = true;
        }
    }

    /// A drag from a press in the field selects to page point `at`.
    fn select_by_drag(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &mut self.field {
            let corner = field.slot.frame.origin;
            let index = field.laid().index_at(at - corner);
            field.select_to(index);
            self.selection_changed(window, cx);
        }
    }

    /// Highlights the `/` menu row under window point `position`. Returns whether the pointer is
    /// over the menu.
    fn light_slash_row(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let (Some((_, menu)), Some(placed)) = (&self.slash, self.slash_frame()) else {
            return false;
        };
        if let Some(row) = slash::row_at(menu, position - placed.origin)
            && !row.lit
        {
            nr_page_slash_light(row.index);
            self.show_slash();
            cx.notify();
        }
        contains(&placed, position)
    }

    /// Highlights the row and button under `position` while the pointer is `over` the page, and
    /// clears them once it leaves.
    fn hover(&mut self, position: Point<Pixels>, over: bool, cx: &mut Context<Self>) {
        let (row, button) = if over {
            let at = self.in_page(position);
            let hit = nr_page_hit(self.key(cx), f64::from(at.x), f64::from(at.y));
            (hit.row, hit.button)
        } else {
            (-1, 0)
        };
        if (over || self.hovering) && nr_page_hover(row, button) {
            cx.notify();
        }
        self.hovering = over;
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.scroller.release(now()) {
            cx.notify();
        }
        self.selecting = false;
        if let Some(following) = self.following.take() {
            let said = page_handle_up(following.shift, &self.typing());
            self.run(said, window, cx);
            return;
        }
        if let (Some((_, menu)), Some(placed)) = (&self.slash, self.slash_frame())
            && let Some(row) = slash::row_at(menu, event.position - placed.origin)
        {
            let command = row.command;
            self.choose_slash(command, window, cx);
        }
    }

    /// A right click on a table cell opens the row and column menu
    /// (`menus::ffi::menu_page_table`). Anywhere else opens no menu.
    fn right_click(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.in_page(event.position);
        let hit = nr_page_hit(self.key(cx), f64::from(at.x), f64::from(at.y));
        let Some(slot) = usize::try_from(hit.cell)
            .ok()
            .and_then(|cell| self.laid.as_ref()?.slots.get(cell).cloned())
        else {
            return;
        };
        // Only a table cell has a row and column (-1 for other slots, and -1 for the title
        // block), so no menu opens elsewhere.
        let (Ok(block), Ok(row), Ok(column)) = (
            usize::try_from(slot.block),
            usize::try_from(slot.row),
            usize::try_from(slot.column),
        ) else {
            return;
        };
        let said = page_table_menu_open(block, row, column, &self.typing());
        self.run(said, window, cx);
        cx.emit(Ask::Menu {
            which: NrMenu::PageTable,
            again: Rc::new(move |_| menu_page_table(block, row)),
            at: event.position,
            below: false,
        });
    }

    /// The wheel scrolls the `/` menu rows when over it, else the page.
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        if let Some(menu) = self.slash_frame()
            && contains(&menu, event.position)
        {
            let (down, unit) = match event.delta {
                ScrollDelta::Pixels(delta) => (-f64::from(delta.y), NR_WHEEL_POINTS),
                ScrollDelta::Lines(delta) => (-f64::from(delta.y), NR_WHEEL_CLICKS),
            };
            nr_page_slash_scroll(down, unit);
            self.show_slash();
            cx.notify();
            return;
        }
        if self.scroller.wheel(event, now()) {
            cx.notify();
        }
    }

    // MARK: Drawing

    /// Whether a core mark draws now. Some draw only while their slot is, or isn't, being edited
    /// (`NrPageMarkShown`).
    fn shows(&self, shown: &NrPageMarkShown, state: &NrPageEditor) -> bool {
        if shown.when == NrMarkWhen::Always {
            return true;
        }
        let Some(slot) = self
            .laid
            .as_ref()
            .and_then(|laid| laid.slots.get(shown.slot as usize))
        else {
            return false;
        };
        let cell = (state.field == 3).then_some((state.row, state.column));
        let typing = self.field.is_some()
            && state.block == slot.block
            && if slot.caption {
                state.field == 4
            } else {
                cell == Some((slot.row, slot.column))
            };
        match shown.when {
            NrMarkWhen::Resting => !typing,
            NrMarkWhen::Typing => typing,
            NrMarkWhen::TypingEmpty => {
                typing
                    && self
                        .field
                        .as_ref()
                        .is_some_and(|field| field.text.is_empty())
            }
            NrMarkWhen::Always => true,
        }
    }

    fn paint(&mut self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        self.lay_out(window, cx);
        let frame = self.frame;
        // A page sits on the panel colour, a shade off the folder background.
        window.paint_quad(gpui::fill(frame, color(Token::Panel)));
        let state = nr_page_editor();
        let key = self.key(cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds: frame }), |window| {
            self.paint_marks(window, cx, &state, key);
        });
        self.listen(hitbox, window, cx);
    }

    /// Paints the marks: overlay marks below the page, the page's visible marks near the frame,
    /// then overlay marks above.
    fn paint_marks(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        state: &NrPageEditor,
        key: usize,
    ) {
        let Some(laid) = &self.laid else { return };
        let (frame, origin) = (self.frame, self.origin());
        let overlay = nr_page_overlay(
            key,
            NrPageOverlayState {
                chosen_first: state.chosen_first,
                chosen_last: state.chosen_last,
                hovered: state.hovered,
                button: state.button,
                drop: state.drop_block,
                drop_after: state.drop_after,
                ringed: if state.media_ring { state.media } else { -1 },
                width: f64::from(laid.width),
            },
        );
        for mark in marks::lent(&overlay.under) {
            marks::paint(window, cx, mark, origin);
        }
        for (mark, shown) in laid.marks() {
            if near_frame(&shown.bounds, origin, frame) && self.shows(shown, state) {
                marks::paint(window, cx, mark, origin);
            }
        }
        for mark in marks::lent(&overlay.over) {
            marks::paint(window, cx, mark, origin);
        }
    }

    /// Paints what sits above the player controls: the text field, the `/` menu and the
    /// scroller.
    fn paint_over(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let now = now();
        let (frame, origin) = (self.frame, self.origin());
        let caret_shown = self.caret_shown && self.field_focus.is_focused(window);
        if let Some(field) = &mut self.field {
            window.with_content_mask(Some(gpui::ContentMask { bounds: frame }), |window| {
                field.paint(window, cx, origin, caret_shown);
            });
            let placed = field.frame();
            let bounds = Bounds {
                origin: origin + placed.origin,
                size: placed.size,
            };
            window.handle_input(
                &self.field_focus,
                ElementInputHandler::new(bounds, cx.entity()),
                cx,
            );
        }
        if let (Some((_, menu)), Some(placed)) = (&self.slash, self.slash_frame()) {
            slash::paint(menu, window, cx, placed.origin);
        }
        self.scroller.paint_bar(window, frame, now);
        if self.scroller.tick(now) {
            window.request_animation_frame();
        }
    }

    /// The window areas the field and `/` menu cover, where player controls ignore presses.
    fn covering(&mut self) -> Vec<Bounds<Pixels>> {
        let origin = self.origin();
        let field = self.field.as_mut().map(|field| {
            let placed = field.frame();
            Bounds {
                origin: origin + placed.origin,
                size: placed.size,
            }
        });
        field.into_iter().chain(self.slash_frame()).collect()
    }

    /// Sets the cursor over the page: a closed hand while dragging a block, else an I-beam while
    /// a field is open with its top-left visible.
    fn set_cursor(&self, hitbox: &Hitbox, window: &mut Window) {
        if self
            .following
            .as_ref()
            .is_some_and(|following| following.dragging)
        {
            window.set_cursor_style(CursorStyle::ClosedHand, hitbox);
        } else if let Some(field) = &self.field {
            let placed = field.slot.frame;
            let over = Bounds {
                origin: self.origin() + placed.origin,
                size: placed.size,
            };
            if contains(&self.frame, over.origin) {
                window.set_cursor_style(CursorStyle::IBeam, hitbox);
            }
        }
    }

    /// Registers pointer handlers while the page is drawn.
    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        self.set_cursor(hitbox, window);
        let page = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            // The page editor decides which view gets focus after a press (`Begin`, `TakeKeys`),
            // not the pressed element.
            window.prevent_default();
            page.update(cx, |page, cx| match event.button {
                MouseButton::Left => page.mouse_down(event, window, cx),
                MouseButton::Right => page.right_click(event, window, cx),
                _ => {}
            });
        });
        let (page, over) = (entity.clone(), hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
            let hovered = over.is_hovered(window);
            page.update(cx, |page, cx| page.mouse_move(event, hovered, window, cx));
        });
        let page = entity.clone();
        on_bubble(window, move |event: &MouseUpEvent, window, cx| {
            if event.button == MouseButton::Left {
                page.update(cx, |page, cx| page.mouse_up(event, window, cx));
            }
        });
        let (page, over) = (entity, hitbox.clone());
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            // A wheel scrolls what is under it even if the keyboard was used last.
            if phase == DispatchPhase::Bubble && over.should_handle_scroll(window) {
                page.update(cx, |page, cx| page.scroll(event, cx));
            }
        });
    }
}

/// Whether a mark at `bounds` (in the page at `origin`) is within 8 pt of `frame`, so it may be
/// visible.
fn near_frame(bounds: &NrRect, origin: Point<Pixels>, frame: Bounds<Pixels>) -> bool {
    let top = f32::from(origin.y) + bounds.y as f32;
    top - 8.0 < f32::from(frame.origin.y + frame.size.height)
        && top + bounds.height as f32 + 8.0 > f32::from(frame.origin.y)
}

/// What a double or triple click at `index` selects: the whole `image` under it, else the word;
/// a triple click selects the line.
fn clicked_selection(
    text: &str,
    image: Option<&Range<usize>>,
    index: usize,
    click_count: usize,
) -> Range<usize> {
    match (image, click_count) {
        (Some(image), 2) => image.clone(),
        (_, 2) => word_at(text, index),
        _ => 0..text.len(),
    }
}

/// The start of the line the caret is on (⌘←).
fn line_start(field: &Field) -> usize {
    let text = field.text.clone();
    text[..field.caret()].rfind('\n').map_or(0, |at| at + 1)
}

/// The end of the line the caret is on (⌘→).
fn line_end(field: &Field) -> usize {
    let caret = field.caret();
    field.text[caret..]
        .find('\n')
        .map_or(field.text.len(), |at| caret + at)
}

/// Runs the editor's answer (`page_menu_answer`) for a menu entry chosen for the page
/// (`NrEffect::Page`).
pub fn menu_chosen(window: &mut Window, cx: &mut App) {
    let Some(page) = LIVE.with(|live| live.borrow().as_ref().and_then(WeakEntity::upgrade)) else {
        return;
    };
    page.update(cx, |page, cx| {
        page.run(page_menu_answer(), window, cx);
    });
}

impl EventEmitter<Ask> for Page {}

impl Screen for Page {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // Reload after the file changed, unless a block is being edited.
        if self.field.is_some() {
            return;
        }
        nr_page_load();
        self.stale = true;
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            if self.frame.size != frame.size {
                self.stale = true;
            }
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, cx: &App) {
        if let Some(keys) = self.has_keys(window, cx) {
            probe.keys = keys.name().to_owned();
        }
    }

    fn scrolled_at(&self, at: Point<Pixels>, _: &App) -> Option<(f32, f32, f32)> {
        let laid = self.laid.as_ref()?;
        contains(&self.frame, at).then(|| {
            let shown = f32::from(self.frame.size.height);
            (self.scroller.offset(), shown, laid.height.max(shown))
        })
    }
}

impl EntityInputHandler for Page {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.field.as_ref()?;
        let range = range_from_utf16(&field.text, &range);
        actual.replace(range_to_utf16(&field.text, &range));
        Some(field.text[range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let field = self.field.as_ref()?;
        Some(UTF16Selection {
            range: range_to_utf16(&field.text, &field.selected),
            reversed: field.reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &mut self.field else { return };
        field.selected = range_from_utf16(&field.text, &range);
        field.reversed = false;
        self.selection_changed(window, cx);
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.field.as_ref()?.text.encode_utf16().count())
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let field = self.field.as_ref()?;
        field
            .marked
            .as_ref()
            .map(|range| range_to_utf16(&field.text, range))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = &mut self.field {
            field.marked = None;
        }
        window.notify_text_input_changed(&self.field_focus);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &self.field else { return };
        let range = range.map(|range| range_from_utf16(&field.text, &range));
        self.type_over(range, text, window, cx);
        self.reveal_caret();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        typed: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(field) = &self.field else { return };
        let range = range
            .map(|range| range_from_utf16(&field.text, &range))
            .or(field.marked.clone())
            .unwrap_or(field.selected.clone());
        // An input method is composing; tell the core and show the composition in the field.
        let over = range_to_utf16(&field.text, &range);
        let said = page_typed(over.start, over.end, typed, true, &self.typing());
        if self.run(said, window, cx).taken {
            return;
        }
        let Some(field) = &mut self.field else { return };
        field.text.replace_range(range.clone(), typed);
        field.marked = (!typed.is_empty()).then(|| range.start..range.start + typed.len());
        field.selected = match selected {
            Some(inside) => {
                range.start + from_utf16(typed, inside.start)
                    ..range.start + from_utf16(typed, inside.end)
            }
            None => range.start + typed.len()..range.start + typed.len(),
        };
        field.reversed = false;
        window.notify_text_input_changed(&self.field_focus);
        self.blink_from_now(cx);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let field = self.field.as_mut()?;
        let range = range_from_utf16(&field.text, &range);
        let placed = field.bounds_of(&range);
        Some(Bounds {
            origin: bounds.origin + placed.origin,
            size: placed.size,
        })
    }

    fn character_index_for_point(
        &mut self,
        at: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let inside = self.in_page(at);
        let field = self.field.as_mut()?;
        let corner = field.slot.frame.origin;
        let index = field.laid().index_at(inside - corner);
        Some(to_utf16(&field.text, index))
    }
}

impl Render for Page {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = cx.entity();
        *self.keys.borrow_mut() = self.has_keys(window, cx);
        let (origin, shown) = (self.origin(), self.frame);
        let covered = self.covering();
        self.players.update(cx, |players, cx| {
            players.set_place(origin, shown, cx);
            players.set_covered(covered, cx);
        });
        let over = cx.entity();
        let mut placed = kit::controls::placed("page", cx.entity_id(), self.frame)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::page_key))
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |_, hitbox, window, cx| {
                        page.update(cx, |page, cx| page.paint(&hitbox, window, cx))
                    },
                )
                .size_full(),
            )
            // The players above the blocks, in window points.
            .child(
                gpui::div()
                    .absolute()
                    .left(-self.frame.origin.x)
                    .top(-self.frame.origin.y)
                    .size_full()
                    .child(self.players.clone()),
            )
            // The field, the `/` menu and the scroller above the players.
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| over.update(cx, |page, cx| page.paint_over(window, cx)),
                )
                .size_full(),
            );
        let origin = self.origin() - self.frame.origin;
        if let Some(field) = &mut self.field {
            // The field's element. It holds focus while a block is edited, and VoiceOver reads it
            // as a text area with the block's written text.
            let at = field.frame();
            let label = if field.slot.block < 0 {
                "Page title"
            } else {
                "Page block"
            };
            placed = placed.child(
                gpui::div()
                    .id("page-field")
                    .absolute()
                    .left(origin.x + at.origin.x)
                    .top(origin.y + at.origin.y)
                    .w(at.size.width)
                    .h(at.size.height)
                    .role(Role::MultilineTextInput)
                    .aria_label(label)
                    // Its text and selection, as `NSTextView` exposes them to VoiceOver.
                    .aria_value(field.text.clone())
                    .a11y_synthetic_children(kit::controls::heard::heard_words(
                        field.text.clone(),
                        field.selected.clone(),
                        field.reversed,
                    ))
                    .key_context(CONTEXT)
                    .track_focus(&self.field_focus)
                    .on_action(cx.listener(Self::move_left))
                    .on_action(cx.listener(Self::move_right))
                    .on_action(cx.listener(Self::move_up))
                    .on_action(cx.listener(Self::move_down))
                    .on_action(cx.listener(Self::select_left))
                    .on_action(cx.listener(Self::select_right))
                    .on_action(cx.listener(Self::select_up))
                    .on_action(cx.listener(Self::select_down))
                    .on_action(cx.listener(Self::word_left))
                    .on_action(cx.listener(Self::word_right))
                    .on_action(cx.listener(Self::select_word_left))
                    .on_action(cx.listener(Self::select_word_right))
                    .on_action(cx.listener(Self::delete_backward))
                    .on_action(cx.listener(Self::delete_word_backward))
                    .on_action(cx.listener(Self::delete_forward))
                    .on_action(cx.listener(Self::newline))
                    .on_action(cx.listener(Self::tab))
                    .on_action(cx.listener(Self::backtab))
                    .on_action(cx.listener(Self::cancel))
                    .on_action(cx.listener(Self::menu_cut))
                    .on_action(cx.listener(Self::menu_copy))
                    .on_action(cx.listener(Self::menu_paste))
                    .on_action(cx.listener(Self::menu_paste_plain))
                    .on_action(cx.listener(Self::menu_select_all))
                    .when(!field.selected.is_empty(), |placed| {
                        placed.on_action(cx.listener(Self::menu_delete))
                    })
                    .on_action(cx.listener(Self::menu_speak))
                    .on_action(cx.listener(Self::menu_stop_speaking))
                    .on_key_down(cx.listener(Self::field_key)),
            );
        }
        placed
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Route::Kind(NrScriptKind::Gutter), gutter);
    table.answer(Route::Kind(NrScriptKind::EditFirst), edit_first);
    table.answer(Route::Kind(NrScriptKind::Media), media);
}

/// The open page, if the window shows one.
fn open_page(cx: &App) -> Option<gpui::Entity<Page>> {
    LIVE.with(|live| live.borrow().as_ref().and_then(WeakEntity::upgrade))
        .filter(|page| page.read(cx).frame.size.width > px(0.0))
}

/// Opens the menu of the page's second block, or presses its `+` (`page_gutter`).
fn gutter(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Done {
    if let Some(page) = open_page(cx) {
        page.update(cx, |page, cx| {
            page.lay_out(window, cx);
            let said = page_gutter(1, step.number == 1, &page.typing());
            page.run(said, window, cx);
        });
    }
    Done::Done
}

/// Starts typing at the end of the first block (`page_edit_first`).
fn edit_first(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Done {
    if let Some(page) = open_page(cx) {
        page.update(cx, |page, cx| {
            page.lay_out(window, cx);
            page.run(page_edit_first(), window, cx);
        });
    }
    Done::Done
}

/// Exercises a board media block's keys and caption field (`page_debug_media`).
fn media(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Done {
    if let Some(page) = open_page(cx) {
        page.update(cx, |page, cx| {
            page.lay_out(window, cx);
            let said = page_debug_media(step.number as usize, &step.name);
            page.run(said, window, cx);
        });
    }
    Done::Done
}
