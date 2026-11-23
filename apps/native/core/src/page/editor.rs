//! The open page's editor state: the field being edited, the selected whole blocks, the focused
//! media block, the `/` menu, the hovered gutter and a handle drag, plus what each press, key
//! and typed character does to them and to the page. It lives here so every shell behaves the
//! same.
//!
//! A shell draws the page, hosts the platform text field for the block being edited, reports
//! what is under a press (`Hit`), names keys (`page::keys`), and passes the field's text and
//! selection with every call (`Typing`, UTF-16 offsets). Each call returns the shell's actions
//! in order (`Answer`): relayout, open a field over a slot, set the field's text or caret, take
//! focus, copy, beep, open a menu. Every page change is its own undo step and is saved
//! (`Pages`).

use crate::list::row_pointer::{drag_starts, Mode, Selected};
use crate::menus::keys::{key_intent, Intent, Row};
use crate::menus::{PageBlockCommand, PageTableCommand};
use crate::page::edit::{
    filter_commands, find_slash_query, takes_shortcut, Caret, Landing, COMMANDS,
};
use crate::page::inline_edit as inline;
use crate::page::keys::{
    blocks_key, blocks_shortcut, cell_move, field_key, media_key, slash_key, BlocksKey,
    BlocksShortcut, CellMove, Command, FieldKey, Held, Key, MediaKey,
};
use crate::page::{BlockKind, Page};
use std::ops::ControlFlow;

/// The open page, as the editor reads and changes it.
pub trait Pages {
    /// Runs `read` on the page; `None` when no page is open.
    fn read<T>(&self, read: impl FnOnce(&Page) -> T) -> Option<T>;
    /// Runs `edit`, which reports whether it changed the page; a change becomes an undo step and is
    /// saved. `None` when no page is open or the page is read-only.
    fn edit<T>(&mut self, edit: impl FnOnce(&mut Page) -> (bool, T)) -> Option<T>;
    /// Retitles the page, renaming its file when the title is the file name. False when refused.
    fn rename(&mut self, title: &str) -> bool;
    /// Undoes one change, or redoes it.
    fn undo(&mut self, redo: bool) -> bool;
    /// Whether the page is read-only: a plain-text file shown as a page.
    fn read_only(&self) -> bool;
}

/// Runs a step that lands the caret, counting it as a change when it changed the file.
fn step<P: Pages>(pages: &mut P, step: impl FnOnce(&mut Page) -> Landing) -> Landing {
    pages
        .edit(|page| {
            let before = page.to_file();
            let landing = step(page);
            (page.to_file() != before, landing)
        })
        .flatten()
}

/// Runs a change that reports whether it changed anything.
fn change<P: Pages>(pages: &mut P, change: impl FnOnce(&mut Page) -> bool) -> bool {
    pages
        .edit(|page| {
            let changed = change(page);
            (changed, changed)
        })
        .unwrap_or(false)
}

/// A block's kind as the editor sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// Editable text: formatted, or verbatim source (code, HTML).
    Words {
        source: bool,
    },
    Divider,
    Table {
        rows: usize,
        columns: usize,
    },
    /// A board media block, focused as a whole; its caption is a separate field.
    Media,
    /// A `<video>` or `<audio>`, selected as a whole and never edited.
    Player,
}

fn shape(page: &Page, block: usize) -> Option<Shape> {
    Some(match &page.blocks.get(block)?.kind {
        BlockKind::Code { .. } | BlockKind::Html { .. } => Shape::Words { source: true },
        BlockKind::Paragraph { .. }
        | BlockKind::Heading { .. }
        | BlockKind::Bullet { .. }
        | BlockKind::Numbered { .. }
        | BlockKind::Todo { .. }
        | BlockKind::Quote { .. } => Shape::Words { source: false },
        BlockKind::Divider => Shape::Divider,
        BlockKind::Table { rows, .. } => Shape::Table {
            rows: rows.len(),
            columns: rows.first().map_or(0, Vec::len),
        },
        BlockKind::Media { .. } => Shape::Media,
        BlockKind::Player { .. } => Shape::Player,
    })
}

/// An editable field, where the page lays out a text slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title,
    Block(usize),
    Cell {
        block: usize,
        row: usize,
        column: usize,
    },
    /// A board media block's caption: one line of plain text.
    Caption(usize),
}

impl Field {
    /// The field's block; `None` for the title.
    pub fn block(self) -> Option<usize> {
        match self {
            Self::Title => None,
            Self::Block(block) | Self::Caption(block) | Self::Cell { block, .. } => Some(block),
        }
    }
}

/// The first field of block `at` (the title for `None`), as the page lays out its slots: a
/// table's first cell, a media block's caption. Dividers and players have none.
fn first_field(page: &Page, at: Option<usize>) -> Option<Field> {
    let Some(block) = at else {
        return Some(Field::Title);
    };
    match shape(page, block)? {
        Shape::Words { .. } => Some(Field::Block(block)),
        Shape::Table { columns, .. } if columns > 0 => Some(Field::Cell {
            block,
            row: 0,
            column: 0,
        }),
        Shape::Media => Some(Field::Caption(block)),
        Shape::Table { .. } | Shape::Divider | Shape::Player => None,
    }
}

/// The page's last field, excluding the title.
fn last_field(page: &Page) -> Option<Field> {
    (0..page.blocks.len())
        .rev()
        .find_map(|block| match shape(page, block)? {
            Shape::Table { rows, columns } if columns > 0 => Some(Field::Cell {
                block,
                row: rows - 1,
                column: columns - 1,
            }),
            _ => first_field(page, Some(block)),
        })
}

/// A field's raw text, inline marks included.
fn field_text(page: &Page, field: Field) -> String {
    match field {
        Field::Title => page.title.clone(),
        Field::Block(block) => page.text(block).unwrap_or_default().to_owned(),
        Field::Cell { block, row, column } => match page.blocks.get(block).map(|block| &block.kind)
        {
            Some(BlockKind::Table { rows, .. }) => rows
                .get(row)
                .and_then(|cells| cells.get(column))
                .cloned()
                .unwrap_or_default(),
            _ => String::new(),
        },
        Field::Caption(block) => match page.blocks.get(block).map(|block| &block.kind) {
            Some(BlockKind::Media { caption, .. }) => caption.clone(),
            _ => String::new(),
        },
    }
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// The shell's field state: its text and selection, in UTF-16 offsets.
#[derive(Clone, Copy, Debug)]
pub struct Typing<'a> {
    pub text: &'a str,
    pub start: usize,
    pub end: usize,
}

impl Typing<'static> {
    /// No field is open.
    pub const NONE: Self = Self {
        text: "",
        start: 0,
        end: 0,
    };
}

/// The block a command applies to, as the field holds it.
struct InBlock<'t> {
    block: usize,
    typing: &'t Typing<'t>,
    /// The field's text as UTF-16 code units, matching its offsets.
    raw: Vec<u16>,
    /// The block holds source (code), where keys behave as in plain text.
    source: bool,
}

impl InBlock<'_> {
    /// The selection is empty: only a caret.
    fn collapsed(&self) -> bool {
        self.typing.start == self.typing.end
    }
}

/// A gutter button.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Button {
    #[default]
    None,
    Plus,
    Handle,
}

/// What is under a press, from the shell's hit test.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hit {
    /// The block whose row, including the gutter, is under the point, and the gutter button there.
    pub row: Option<usize>,
    pub button: Button,
    /// The block whose frame contains the point.
    pub block: Option<usize>,
    /// The media block whose frame contains the point.
    pub media: Option<usize>,
    /// The to-do whose box, with 4 pt of slop, contains the point.
    pub todo: Option<usize>,
    /// The text slot under the point, with 2 pt of slop.
    pub slot: Option<Field>,
    /// The point is below the page's last text.
    pub below: bool,
}

/// An action the shell performs for the editor, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Close the field; its contents are already saved.
    EndEdit,
    /// Lay out the page again from its current state.
    Relayout,
    /// Open a field over the slot of `block` (-1 for the title), `row` and `column` (-1 except in a
    /// table cell), or its caption, holding `text` with the caret at `caret`.
    Begin {
        block: i64,
        row: i64,
        column: i64,
        caption: bool,
        text: String,
        caret: usize,
    },
    /// Set the field's text and caret as a core edit left them.
    Apply {
        text: String,
        caret: usize,
    },
    /// Set the field's text and caret as user typing would.
    Retype {
        text: String,
        caret: usize,
    },
    /// Set the field's selection without side effects.
    Place {
        start: usize,
        end: usize,
    },
    /// Set the field's selection as a user selection would.
    Select {
        start: usize,
        end: usize,
    },
    /// Insert `text` over `start..end` of the field, as typed.
    Insert {
        text: String,
        start: usize,
        end: usize,
    },
    /// Restyle the field's text (`ffi::inline_style`).
    Restyle,
    /// For a check: the field reports its contents (`NR_PAGE_TRACE`).
    Trace {
        step: &'static str,
    },
    /// The page takes keyboard focus.
    TakeKeys,
    Beep,
    /// Copy `text` to the clipboard.
    Copy {
        text: String,
    },
    /// Paste the clipboard's text after the selected blocks (`Editor::paste_blocks`).
    Paste,
    /// The field moves its caret a line up or down itself, then reports where it went
    /// (`Editor::vertical`).
    Vertical {
        up: bool,
    },
    /// Open the block's menu under its row (`menus::page_block_menu`).
    BlockMenu {
        block: usize,
    },
    /// The `/` menu opened, changed or closed (`Editor::slash`).
    Slash,
    /// The page ignores the key; pass it on.
    Pass,
    /// The press is on a handle; track it as a drag (`Editor::handle_move`).
    Drag,
    /// Forward the press to the newly opened field, which places the caret there.
    Forward,
}

/// A call's result: whether the page handled its input (a key, a command, typed text), and
/// what the shell must do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub taken: bool,
    pub effects: Vec<Effect>,
}

/// The `/` menu while it is open.
#[derive(Clone, Debug, PartialEq)]
pub struct Slash {
    pub block: usize,
    /// The slash's offset in the line (UTF-16 units).
    pub start: usize,
    /// The entries shown, as indices into `page::edit::COMMANDS`.
    pub shown: Vec<usize>,
    /// The highlighted entry.
    pub lit: usize,
    /// The rows' scroll offset (`menus::slash::scroll`), keeping the highlighted one in view.
    pub offset: f64,
}

/// A press on a block's handle, until the button comes up.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Handle {
    block: usize,
    /// The blocks a drag moves: the selection the block is in, else the block alone.
    first: usize,
    last: usize,
    dragging: bool,
}

#[derive(Debug, Default)]
pub struct Editor {
    pub editing: Option<Field>,
    /// The selected whole blocks: contiguous, with the anchor at one end.
    pub chosen: Option<Selected>,
    /// The focused media block, and whether it was reached by keyboard, which alone shows its ring
    /// (`:focus-visible`).
    pub media: Option<(usize, bool)>,
    pub slash: Option<Slash>,
    /// The block whose gutter shows, and the hovered button.
    pub hovered: Option<usize>,
    pub button: Button,
    /// Where dragged blocks would land: before or after (`true`) the block.
    pub drop: Option<(usize, bool)>,
    menu_block: Option<usize>,
    menu_cell: Option<(usize, usize, usize)>,
    handle: Option<Handle>,
    effects: Vec<Effect>,
}

impl Editor {
    pub const fn new() -> Self {
        Self {
            editing: None,
            chosen: None,
            media: None,
            slash: None,
            hovered: None,
            button: Button::None,
            drop: None,
            menu_block: None,
            menu_cell: None,
            handle: None,
            effects: Vec::new(),
        }
    }

    fn emit(&mut self, effect: Effect) {
        self.effects.push(effect);
    }

    fn answer(&mut self, taken: bool) -> Answer {
        Answer {
            taken,
            effects: std::mem::take(&mut self.effects),
        }
    }

    /// The selected blocks: the first, the last and the anchor.
    pub fn chosen_range(&self) -> Option<(usize, usize, usize)> {
        let chosen = self.chosen.as_ref()?;
        let (first, last) = (*chosen.rows.first()?, *chosen.rows.last()?);
        Some((first, last, chosen.anchor.unwrap_or(first)))
    }

    /// Whether the field is a block's formatted text, not code, a table cell, a caption or the
    /// title.
    pub fn formatted<P: Pages>(&self, pages: &P) -> bool {
        match self.editing {
            Some(Field::Block(block)) => pages
                .read(|page| shape(page, block) == Some(Shape::Words { source: false }))
                .unwrap_or(false),
            _ => false,
        }
    }

    // Shared helpers ---------------------------------------------------------------------------

    /// Selects only blocks `first..=last`, anchored at `anchor`, and gives the page focus.
    fn choose(&mut self, first: usize, last: usize, anchor: usize) {
        self.media = None;
        let focus = if anchor == first { last } else { first };
        self.chosen = Some(Selected {
            rows: (first..=last).collect(),
            anchor: Some(anchor),
            focus: Some(focus),
        });
        self.emit(Effect::TakeKeys);
    }

    /// Selects blocks `from` to `to`, anchored at `from` (`Selected::press`).
    fn reach<P: Pages>(&mut self, pages: &P, from: usize, to: usize) {
        let count = pages.read(|page| page.blocks.len()).unwrap_or(0);
        self.media = None;
        self.chosen = Some(
            Selected {
                rows: Vec::new(),
                anchor: Some(from),
                focus: None,
            }
            .press(count, to, Mode::Range),
        );
        self.emit(Effect::TakeKeys);
    }

    /// The anchor for a shift-press range: the selection's anchor, else the block being edited,
    /// else the pressed block.
    fn range_from(&self, pressed: usize) -> usize {
        self.chosen_range()
            .map(|(_, _, anchor)| anchor)
            .or_else(|| self.editing.and_then(Field::block))
            .unwrap_or(pressed)
    }

    fn focus_media(&mut self, block: usize, ring: bool) {
        self.chosen = None;
        self.media = Some((block, ring));
        self.emit(Effect::TakeKeys);
    }

    /// Opens a field over `field` with the page's text and the caret at `caret`.
    fn begin<P: Pages>(&mut self, pages: &P, field: Field, caret: Caret) {
        if pages.read_only() {
            return;
        }
        let Some(text) = pages.read(|page| field_text(page, field)) else {
            return;
        };
        let length = text.encode_utf16().count();
        let caret = match caret {
            Caret::Start => 0,
            Caret::End => length,
            Caret::At(at) => at.min(length),
        };
        self.media = None;
        self.editing = Some(field);
        let (block, row, column) = match field {
            Field::Title => (-1, -1, -1),
            Field::Block(block) | Field::Caption(block) => (block as i64, -1, -1),
            Field::Cell { block, row, column } => (block as i64, row as i64, column as i64),
        };
        self.emit(Effect::Begin {
            block,
            row,
            column,
            caption: matches!(field, Field::Caption(_)),
            text,
            caret,
        });
    }

    /// Relayouts and moves to block `at` (the title for `None`): a media block gets focus, a player
    /// is selected whole, anything else is edited at `caret`.
    fn edit<P: Pages>(&mut self, pages: &P, at: Option<usize>, caret: Caret) {
        self.emit(Effect::Relayout);
        if pages.read_only() {
            return;
        }
        let (found, field) = pages
            .read(|page| {
                (
                    at.and_then(|block| shape(page, block)),
                    first_field(page, at),
                )
            })
            .unwrap_or((None, None));
        match (at, found, field) {
            (Some(block), Some(Shape::Media), _) => self.focus_media(block, true),
            (Some(block), Some(Shape::Player), _) => self.choose(block, block, block),
            (_, _, Some(field)) => self.begin(pages, field, caret),
            _ => {}
        }
    }

    /// Moves to where an editing step landed the caret.
    fn land<P: Pages>(&mut self, pages: &P, landing: Landing) {
        match landing {
            Some((block, caret)) => self.edit(pages, Some(block), caret),
            None => self.emit(Effect::Relayout),
        }
    }

    /// Closes the field and saves its contents, without relayout.
    fn store<P: Pages>(&mut self, pages: &mut P, text: &str) {
        let Some(field) = self.editing.take() else {
            return;
        };
        self.emit(Effect::EndEdit);
        self.close_slash();
        match field {
            Field::Caption(block) => {
                change(pages, |page| page.set_caption(block, text));
            }
            Field::Cell { block, row, column } => {
                change(pages, |page| page.set_table_cell(block, row, column, text));
            }
            // A page whose file has a title line keeps its file name; otherwise the title is the
            // file name, so the file is renamed.
            Field::Title => {
                if !pages.rename(text) {
                    self.emit(Effect::Beep);
                }
            }
            Field::Block(block) => {
                pages.edit(|page| (page.set_text(block, text), ()));
            }
        }
    }

    /// Ends editing and saves the text.
    fn commit<P: Pages>(&mut self, pages: &mut P, text: &str) {
        self.close_slash();
        if self.editing.is_none() {
            return;
        }
        self.store(pages, text);
        self.emit(Effect::Relayout);
        self.emit(Effect::Trace { step: "commit" });
    }

    fn close_slash(&mut self) {
        if self.slash.take().is_some() {
            self.emit(Effect::Slash);
        }
    }

    /// Updates `block`'s `/` menu for `query`, typed after the slash at `start`: shows the
    /// matching entries, highlighting the first when they changed.
    fn show_slash(&mut self, block: usize, start: usize, query: &str) {
        let shown = filter_commands(query);
        let (lit, offset) = match &self.slash {
            Some(open) if open.shown == shown => (open.lit, open.offset),
            _ => (0, 0.0),
        };
        self.slash = Some(Slash {
            block,
            start,
            shown,
            lit,
            offset,
        });
        self.emit(Effect::Slash);
    }

    fn open_block_menu(&mut self, block: usize) {
        self.menu_block = Some(block);
        self.emit(Effect::BlockMenu { block });
    }

    // The pointer ------------------------------------------------------------------------------

    /// The pointer is over block `row`'s row (or none) and gutter `button`. Returns whether the
    /// display changed. A read-only page has no gutter.
    pub fn hover<P: Pages>(&mut self, pages: &P, row: Option<usize>, button: Button) -> bool {
        let (row, button) = if pages.read_only() {
            (None, Button::None)
        } else {
            (row, button)
        };
        let changed = (self.hovered, self.button) != (row, button);
        self.hovered = row;
        self.button = button;
        changed
    }

    /// A press on the page, tried in order: the gutter's `+` and handle; with ⇧, a range from the
    /// edited or selected block to the pressed one; then, after ending editing, a player block is
    /// selected, a media block takes focus, a to-do toggles, a text slot is edited (the press is
    /// forwarded to its field), or below the last block editing continues in an empty last line.
    pub fn press<P: Pages>(
        &mut self,
        pages: &mut P,
        hit: Hit,
        shift: bool,
        typing: &Typing,
    ) -> Answer {
        if pages.read_only() {
            return self.answer(false);
        }
        self.hover(pages, hit.row, hit.button);
        if let Some(row) = hit.row {
            match hit.button {
                Button::Plus => {
                    self.add_block(pages, row, typing);
                    return self.answer(true);
                }
                Button::Handle => {
                    let (moving, _) = self.chosen.clone().unwrap_or_default().pick_up(row);
                    let (first, last) = (moving[0], moving[moving.len() - 1]);
                    self.handle = Some(Handle {
                        block: row,
                        first,
                        last,
                        dragging: false,
                    });
                    self.emit(Effect::Drag);
                    return self.answer(true);
                }
                Button::None => {}
            }
        }
        if let (true, Some(pressed)) = (shift, hit.block) {
            let from = self.range_from(pressed);
            self.commit(pages, typing.text);
            self.reach(pages, from, pressed);
            return self.answer(true);
        }
        self.chosen = None;
        self.media = None;
        self.commit(pages, typing.text);
        let player = hit.block.filter(|&block| {
            pages
                .read(|page| shape(page, block) == Some(Shape::Player))
                .unwrap_or(false)
        });
        if let Some(block) = player {
            // A press on a player outside its controls selects the block; its HTML is never
            // opened for editing.
            self.choose(block, block, block);
        } else if let Some(block) = hit.media {
            self.focus_media(block, false);
        } else if let Some(block) = hit.todo {
            pages.edit(|page| (page.toggle_todo(block), ()));
            self.emit(Effect::Relayout);
        } else if let Some(field) = hit.slot {
            self.begin(pages, field, Caret::End);
            self.emit(Effect::Forward);
        } else if hit.below {
            // An empty line is not written to the file, so it is not a change to save or undo.
            let line = pages
                .edit(|page| (false, page.ensure_trailing_paragraph()))
                .unwrap_or(0);
            self.edit(pages, Some(line), Caret::Start);
        }
        self.answer(true)
    }

    /// The `+`: adds an empty line below block `row` with the `/` menu open, as if `/` were typed.
    fn add_block<P: Pages>(&mut self, pages: &mut P, row: usize, typing: &Typing) {
        self.chosen = None;
        self.commit(pages, typing.text);
        let added = pages
            .edit(|page| (false, page.insert_after(row)))
            .unwrap_or(0);
        self.edit(pages, Some(added), Caret::Start);
        if self.editing == Some(Field::Block(added)) {
            self.emit(Effect::Retype {
                text: "/".to_owned(),
                caret: 1,
            });
            self.show_slash(added, 0, "");
        }
    }

    /// A handle press has moved (`dx`, `dy`) points on a screen of `scale` pixels per point. A drag
    /// starts at Chromium's threshold (`row_pointer::drag_starts`), ending editing, and targets
    /// where `drop_at` says blocks `first..=last` would land (`page::layout::drop_at`: beside the
    /// block under the pointer, never on the dragged blocks). Returns whether the press is a drag.
    pub fn handle_move<P: Pages>(
        &mut self,
        pages: &mut P,
        dx: f64,
        dy: f64,
        scale: f64,
        drop_at: impl FnOnce(usize, usize) -> Option<(usize, bool)>,
        typing: &Typing,
    ) -> Answer {
        let Some(mut handle) = self.handle.clone() else {
            return self.answer(false);
        };
        if !handle.dragging && drag_starts(dx, dy, scale) {
            handle.dragging = true;
            self.handle = Some(handle.clone());
            self.commit(pages, typing.text);
        }
        if !handle.dragging {
            return self.answer(false);
        }
        self.drop = drop_at(handle.first, handle.last);
        self.answer(true)
    }

    /// A handle press ends: a drag drops its blocks at the target; a shift-click extends the
    /// selection to the block; a click selects only it and opens its menu. Returns whether the
    /// press was a drag.
    pub fn handle_up<P: Pages>(&mut self, pages: &mut P, shift: bool, typing: &Typing) -> Answer {
        let Some(handle) = self.handle.take() else {
            return self.answer(false);
        };
        if handle.dragging {
            if let Some((target, after)) = self.drop.take() {
                self.chosen = None;
                pages.edit(|page| {
                    let moved = page.move_next_to(handle.first, handle.last, target, after);
                    (moved.is_some(), ())
                });
                self.emit(Effect::Relayout);
            }
            return self.answer(true);
        }
        let from = self.range_from(handle.block);
        self.commit(pages, typing.text);
        if shift {
            self.reach(pages, from, handle.block);
        } else {
            self.choose(handle.block, handle.block, handle.block);
            self.open_block_menu(handle.block);
        }
        self.answer(false)
    }

    /// Handles a chosen block menu entry.
    pub fn block_menu<P: Pages>(&mut self, pages: &mut P, command: PageBlockCommand) -> Answer {
        let Some(block) = self.menu_block else {
            return self.answer(false);
        };
        match command {
            PageBlockCommand::Duplicate => {
                change(pages, |page| page.duplicate(block, block));
                self.emit(Effect::Relayout);
                self.choose(block + 1, block + 1, block + 1);
            }
            PageBlockCommand::Delete => {
                self.chosen = None;
                step(pages, |page| page.delete_range(block, block));
                self.emit(Effect::Relayout);
            }
            PageBlockCommand::Turn(command) => {
                self.chosen = None;
                let landing = step(pages, |page| page.set_kind(block, COMMANDS[command].kind));
                self.land(pages, landing);
            }
        }
        self.answer(true)
    }

    /// A right click on cell `row`, `column` of table `block`: ends editing and targets the cell
    /// menu at that cell.
    pub fn open_table_menu<P: Pages>(
        &mut self,
        pages: &mut P,
        block: usize,
        row: usize,
        column: usize,
        typing: &Typing,
    ) -> Answer {
        self.commit(pages, typing.text);
        self.menu_cell = Some((block, row, column));
        self.answer(true)
    }

    /// Handles a chosen table cell menu entry.
    pub fn table_menu<P: Pages>(&mut self, pages: &mut P, command: PageTableCommand) -> Answer {
        let Some((block, row, column)) = self.menu_cell else {
            return self.answer(false);
        };
        change(pages, |page| match command {
            PageTableCommand::AddRow => page.add_table_row(block, row),
            PageTableCommand::AddColumn => page.add_table_column(block),
            PageTableCommand::DeleteRow => page.delete_table_row(block, row),
            PageTableCommand::DeleteColumn => page.delete_table_column(block, column),
        });
        self.emit(Effect::Relayout);
        self.answer(true)
    }

    // Keys on the page itself ------------------------------------------------------------------

    /// A key while the page has focus and no field is open: a focused media block's keys, then the
    /// selected blocks'. Any other key passes on (`Effect::Pass`).
    pub fn key<P: Pages>(&mut self, pages: &mut P, key: Key, held: Held) -> Answer {
        if self.chosen.is_none() {
            if let (Some((block, _)), Some(intent)) = (self.media, media_key(key, held)) {
                self.media_key(pages, intent, block);
                return self.answer(true);
            }
        }
        match (self.chosen.is_some(), blocks_key(key, held)) {
            (true, Some(intent)) => self.blocks_key(pages, intent),
            _ => self.emit(Effect::Pass),
        }
        self.answer(true)
    }

    /// A shortcut while the page has focus: the selected blocks handle theirs before the menu bar.
    /// Returns whether they did.
    pub fn shortcut<P: Pages>(&mut self, pages: &mut P, key: Key, held: Held) -> Answer {
        if self.chosen.is_none() {
            return self.answer(false);
        }
        match blocks_shortcut(key, held) {
            None => return self.answer(false),
            Some(BlocksShortcut::Undo { redo }) => {
                self.chosen = None;
                pages.undo(redo);
                self.emit(Effect::Relayout);
            }
            Some(BlocksShortcut::Key) => match blocks_key(key, held) {
                Some(intent) => self.blocks_key(pages, intent),
                None => self.emit(Effect::Pass),
            },
        }
        self.answer(true)
    }

    fn media_key<P: Pages>(&mut self, pages: &mut P, intent: MediaKey, block: usize) {
        match intent {
            MediaKey::Remove => {
                self.media = None;
                let landing = step(pages, |page| page.delete_range(block, block));
                self.land(pages, landing);
            }
            MediaKey::LineBelow => {
                self.media = None;
                let added = pages
                    .edit(|page| (false, page.insert_after(block)))
                    .unwrap_or(0);
                self.edit(pages, Some(added), Caret::Start);
            }
            MediaKey::Step { up } => {
                let count = pages.read(|page| page.blocks.len()).unwrap_or(0);
                let neighbour = if up {
                    block.checked_sub(1)
                } else {
                    Some(block + 1)
                };
                if let Some(neighbour) = neighbour.filter(|&neighbour| neighbour < count) {
                    self.edit(
                        pages,
                        Some(neighbour),
                        if up { Caret::End } else { Caret::Start },
                    );
                }
            }
        }
    }

    fn blocks_key<P: Pages>(&mut self, pages: &mut P, intent: BlocksKey) {
        let Some((first, last, anchor)) = self.chosen_range() else {
            return;
        };
        let moved = last - first + 1;
        let count = pages.read(|page| page.blocks.len()).unwrap_or(0);
        let end = count.saturating_sub(1);
        // Extending the selection moves the end opposite the anchor.
        let moving = if first == anchor { last } else { first };
        match intent {
            BlocksKey::Delete => {
                self.chosen = None;
                let landing = step(pages, |page| page.delete_range(first, last));
                self.emit(Effect::Relayout);
                if let Some((block, _)) = landing {
                    self.choose(block, block, block);
                }
            }
            BlocksKey::Duplicate => {
                change(pages, |page| page.duplicate(first, last));
                self.emit(Effect::Relayout);
                self.choose(first + moved, last + moved, first + moved);
            }
            BlocksKey::Move { up } => {
                let to = pages
                    .edit(|page| {
                        let to = page.move_by_step(first, last, up);
                        (to.is_some(), to)
                    })
                    .flatten();
                self.emit(Effect::Relayout);
                if let Some(to) = to {
                    self.choose(to, to + moved - 1, to);
                }
            }
            BlocksKey::Step { up: true } => {
                let next = first.saturating_sub(1).min(end);
                self.choose(next, next, next);
            }
            BlocksKey::Step { up: false } => {
                let next = (last + 1).min(end);
                self.choose(next, next, next);
            }
            BlocksKey::Grow { up: true } => {
                self.grow_choice(count, moving.saturating_sub(1).min(end))
            }
            BlocksKey::Grow { up: false } => self.grow_choice(count, (moving + 1).min(end)),
            BlocksKey::Edit => self.edit_chosen(pages, first, moved),
            BlocksKey::Clear => self.chosen = None,
            BlocksKey::Copy => self.copy_blocks(pages, first, last),
            BlocksKey::Cut => {
                self.copy_blocks(pages, first, last);
                self.chosen = None;
                step(pages, |page| page.delete_range(first, last));
                self.emit(Effect::Relayout);
            }
            BlocksKey::Paste => self.emit(Effect::Paste),
            BlocksKey::SelectAll => self.choose(0, end, 0),
        }
    }

    /// Extends or shrinks the selection, among `count` blocks, to `next`.
    fn grow_choice(&mut self, count: usize, next: usize) {
        self.chosen = self
            .chosen
            .as_ref()
            .map(|chosen| chosen.press(count, next, Mode::Range));
    }

    /// Return on the `moved` selected blocks from `first` edits the first; a lone player has no
    /// text, so a new line starts below it.
    fn edit_chosen<P: Pages>(&mut self, pages: &mut P, first: usize, moved: usize) {
        self.chosen = None;
        let player = pages
            .read(|page| shape(page, first) == Some(Shape::Player))
            .unwrap_or(false);
        if moved == 1 && player {
            let added = pages
                .edit(|page| (false, page.insert_after(first)))
                .unwrap_or(0);
            self.edit(pages, Some(added), Caret::Start);
        } else {
            self.edit(pages, Some(first), Caret::End);
        }
    }

    /// Copies blocks `first` to `last` to the clipboard as Markdown.
    fn copy_blocks<P: Pages>(&mut self, pages: &mut P, first: usize, last: usize) {
        let markdown = pages
            .read(|page| page.markdown_of(first, last))
            .unwrap_or_default();
        self.emit(Effect::Copy { text: markdown });
    }

    /// Pastes the clipboard's Markdown as blocks after the selected ones, then selects the pasted
    /// blocks.
    pub fn paste_blocks<P: Pages>(&mut self, pages: &mut P, markdown: &str) -> Answer {
        let Some((_, last, _)) = self.chosen_range() else {
            return self.answer(false);
        };
        let pasted = pages
            .edit(|page| {
                let pasted = page.insert_markdown_after(last, markdown);
                (pasted.is_some(), pasted)
            })
            .flatten();
        self.emit(Effect::Relayout);
        if let Some((_, end)) = pasted {
            self.choose(last + 1, end, last + 1);
        }
        self.answer(true)
    }

    // Keys while a field is open -----------------------------------------------------------------

    /// A shortcut while the field has focus: formatting, block kinds, undo across blocks, and the
    /// block's own shortcuts. Captions and the title take only undo. Returns whether the page
    /// handled it.
    pub fn field_shortcut<P: Pages>(
        &mut self,
        pages: &mut P,
        key: Key,
        held: Held,
        typing: &Typing,
    ) -> Answer {
        let (Some(field), Some(intent)) = (self.editing, field_key(key, held)) else {
            return self.answer(false);
        };
        let (start, end) = (typing.start, typing.end);
        let words = self.formatted(pages);
        match (intent, field) {
            (FieldKey::Undo { redo }, _) => {
                self.store(pages, typing.text);
                pages.undo(redo);
                self.emit(Effect::Relayout);
                let at = field.block();
                if let Some(again) = pages
                    .read(|page| first_field(page, at).or_else(|| last_field(page)))
                    .flatten()
                {
                    self.begin(pages, again, Caret::End);
                }
            }
            (_, Field::Caption(_)) => return self.answer(false),
            (FieldKey::Mark(mark), Field::Block(block)) if words => {
                self.store(pages, typing.text);
                let chosen = pages
                    .edit(|page| {
                        let toggled = page.toggle_mark(block, start, end, mark);
                        (toggled.is_some(), toggled)
                    })
                    .flatten();
                self.edit(pages, Some(block), Caret::At(start));
                if let Some((start, end)) = chosen {
                    self.emit(Effect::Select { start, end });
                }
            }
            (FieldKey::Link, Field::Block(block)) if words => {
                self.store(pages, typing.text);
                let landed = pages
                    .edit(|page| {
                        let before = page.to_file();
                        let caret = page.link_selection(block, start, end);
                        (page.to_file() != before, caret)
                    })
                    .flatten();
                self.edit(pages, Some(block), Caret::At(landed.unwrap_or(start)));
            }
            (FieldKey::Turn(command), Field::Block(block)) if words => {
                self.store(pages, typing.text);
                let landing = step(pages, |page| page.set_kind(block, COMMANDS[command].kind));
                self.land(pages, landing);
            }
            (FieldKey::Move { up }, Field::Block(block) | Field::Cell { block, .. }) => {
                self.store(pages, typing.text);
                let moved = pages
                    .edit(|page| {
                        let moved = page.move_by_step(block, block, up);
                        (moved.is_some(), moved)
                    })
                    .flatten();
                self.edit(pages, Some(moved.unwrap_or(block)), Caret::At(start));
            }
            (FieldKey::Duplicate, Field::Block(block) | Field::Cell { block, .. }) => {
                self.store(pages, typing.text);
                change(pages, |page| page.duplicate(block, block));
                self.edit(pages, Some(block + 1), Caret::At(start));
            }
            (FieldKey::ToggleTodo, Field::Block(block) | Field::Cell { block, .. }) => {
                self.store(pages, typing.text);
                pages.edit(|page| (page.toggle_todo(block), ()));
                self.edit(pages, Some(block), Caret::At(start));
            }
            _ => return self.answer(false),
        }
        self.answer(true)
    }

    /// A command from the field's text view (`page::keys::Command`). The `/` menu's keys come
    /// first; a caption keeps Return, Tab and Escape from the page; Escape selects the block; a
    /// cell moves between cells; the title's Return goes to the first block; Return splits; ← and →
    /// skip whole marks and images; Backspace and Delete remove one visible character or join
    /// lines; Tab indents; ↑ and ↓ leave the block at its edges. Returns whether the page handled
    /// it. With `Effect::Vertical` the field moves first and `vertical` finishes the job.
    pub fn command<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        typing: &Typing,
    ) -> Answer {
        if self.slash_command(pages, command, typing) {
            return self.answer(true);
        }
        let Some(field) = self.editing else {
            return self.answer(false);
        };
        let block = match self.field_command(pages, command, typing, field) {
            ControlFlow::Break(answer) => return answer,
            ControlFlow::Continue(block) => block,
        };
        let source = pages
            .read(|page| shape(page, block) == Some(Shape::Words { source: true }))
            .unwrap_or(false);
        let at = InBlock {
            block,
            typing,
            raw: utf16(typing.text),
            source,
        };
        if let Some(answer) = self.block_command(pages, command, &at) {
            return answer;
        }
        if let Some(answer) = self.leave_line(pages, command, &at) {
            return answer;
        }
        self.answer(false)
    }

    /// Field-level handling of `command` before the block's: the answer when the field handles it,
    /// else the block it is in.
    fn field_command<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        typing: &Typing,
        field: Field,
    ) -> ControlFlow<Answer, usize> {
        match field {
            // The caption is an input field, so Return, Tab and Escape do nothing to the page.
            Field::Caption(_) => ControlFlow::Break(self.answer(matches!(
                command,
                Command::Newline | Command::Tab | Command::Backtab | Command::Cancel
            ))),
            _ if command == Command::Cancel => {
                // Escape leaves the text and selects its block.
                self.commit(pages, typing.text);
                if let Some(block) = field.block() {
                    self.choose(block, block, block);
                }
                ControlFlow::Break(self.answer(true))
            }
            Field::Cell { block, row, column } => {
                ControlFlow::Break(self.cell_command(pages, command, block, row, column, typing))
            }
            Field::Title if command == Command::Newline => {
                // The title is one line, so Return saves it and moves to the first block.
                self.store(pages, typing.text);
                self.edit(pages, Some(0), Caret::Start);
                ControlFlow::Break(self.answer(true))
            }
            Field::Title => ControlFlow::Break(self.answer(false)),
            Field::Block(block) => ControlFlow::Continue(block),
        }
    }

    /// Block-level handling of `command`: the answer when the block handles or rejects it.
    fn block_command<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        at: &InBlock,
    ) -> Option<Answer> {
        let (block, text, start) = (at.block, at.typing.text, at.typing.start);
        let (source, collapsed) = (at.source, at.collapsed());
        match command {
            // Return splits the block at the caret, except in code, where it inserts a newline.
            Command::Newline if source => Some(self.answer(false)),
            Command::Newline => {
                self.store(pages, text);
                let landing = step(pages, |page| page.split(block, start, at.typing.end));
                self.land(pages, landing);
                Some(self.answer(true))
            }
            Command::Left | Command::Right if !source && collapsed => {
                Some(self.step_caret(command, at))
            }
            Command::DeleteBackward | Command::DeleteForward if !source && collapsed => {
                Some(self.delete_shown(pages, command, at))
            }
            Command::DeleteBackward if start == 0 && collapsed => {
                self.merge(pages, block, true, text);
                Some(self.answer(true))
            }
            Command::DeleteForward if start == at.raw.len() && collapsed => {
                self.merge(pages, block, false, text);
                Some(self.answer(true))
            }
            Command::Tab | Command::Backtab if source => Some(self.answer(false)),
            Command::Tab | Command::Backtab => {
                self.store(pages, text);
                change(pages, |page| page.indent(block, command == Command::Tab));
                self.edit(pages, Some(block), Caret::At(start));
                Some(self.answer(true))
            }
            Command::Up if start == 0 && block > 0 => {
                self.store(pages, text);
                // Skip players above, as ↓ does, moving the caret to the text above.
                let above = pages
                    .read(|page| {
                        (0..block)
                            .rev()
                            .find(|&above| shape(page, above) != Some(Shape::Player))
                    })
                    .flatten();
                self.edit(pages, above, Caret::End);
                Some(self.answer(true))
            }
            _ => None,
        }
    }

    /// ← and → move past one visible character and the hidden marks around it.
    fn step_caret(&mut self, command: Command, at: &InBlock) -> Answer {
        let Some(caret) = inline::step(&at.raw, at.typing.start, command == Command::Right) else {
            return self.answer(false);
        };
        self.emit(Effect::Place {
            start: caret,
            end: caret,
        });
        self.emit(Effect::Restyle);
        self.emit(Effect::Trace { step: "move" });
        self.answer(true)
    }

    /// Backspace and Delete remove one visible character; with none left on that side, the block's
    /// formatting is removed or the line joins its neighbour.
    fn delete_shown<P: Pages>(&mut self, pages: &mut P, command: Command, at: &InBlock) -> Answer {
        let backward = command == Command::DeleteBackward;
        if let Some(range) = inline::delete_unit(&at.raw, at.typing.start, backward) {
            let edit = inline::replace_range(&at.raw, range, &[]);
            self.emit(Effect::Apply {
                text: String::from_utf16_lossy(&edit.text),
                caret: edit.caret,
            });
        } else {
            self.merge(pages, at.block, backward, at.typing.text);
        }
        self.answer(true)
    }

    /// ↑ and ↓ that leave the caret's line: onto an image's line, or out of the block's end.
    fn leave_line<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        at: &InBlock,
    ) -> Option<Answer> {
        let (start, length) = (at.typing.start, at.raw.len());
        let vertical = matches!(command, Command::Up | Command::Down);
        let at_edge =
            (command == Command::Up && start == 0) || (command == Command::Down && start == length);
        if vertical
            && !at.source
            && at.collapsed()
            && !at_edge
            && !inline::image_spans(&at.raw).is_empty()
        {
            // Up or down onto an image's line lands beside it, on the side it was reached from.
            self.emit(Effect::Vertical {
                up: command == Command::Up,
            });
            return Some(self.answer(true));
        }
        if command == Command::Down && start == length {
            self.go_below(pages, at);
            return Some(self.answer(true));
        }
        None
    }

    /// ↓ at the end of a block: to the media block below, else to the next text below, else to the
    /// end of its own.
    fn go_below<P: Pages>(&mut self, pages: &mut P, at: &InBlock) {
        let block = at.block;
        self.store(pages, at.typing.text);
        self.emit(Effect::Relayout);
        let next = pages.read(|page| {
            if shape(page, block + 1) == Some(Shape::Media) {
                return (Some(block + 1), None);
            }
            let below =
                (block + 1..page.blocks.len()).find_map(|below| first_field(page, Some(below)));
            (
                None,
                below
                    .map(|field| (field, Caret::Start))
                    .or_else(|| first_field(page, Some(block)).map(|field| (field, Caret::End))),
            )
        });
        match next {
            Some((Some(media), _)) => self.focus_media(media, true),
            Some((None, Some((field, caret)))) => self.begin(pages, field, caret),
            _ => {}
        }
    }

    fn merge<P: Pages>(&mut self, pages: &mut P, block: usize, backward: bool, text: &str) {
        self.store(pages, text);
        let landing = step(pages, |page| {
            if backward {
                page.merge_backward(block)
            } else {
                page.merge_forward(block)
            }
        });
        self.land(pages, landing);
    }

    /// The field moved its caret a line up (`up`) or down from `from` (`Effect::Vertical`) to
    /// `typing.start`. A caret that landed inside an image moves beside it, on the side it came
    /// from. Returns whether the caret moved.
    pub fn vertical(&mut self, from: usize, up: bool, typing: &Typing) -> Answer {
        let raw = utf16(typing.text);
        let mut landed = typing.start;
        if let Some((lower, upper)) = inline::image_spans(&raw)
            .into_iter()
            .find(|&(lower, upper)| lower < landed && landed < upper)
        {
            landed = if up { lower } else { upper };
            self.emit(Effect::Place {
                start: landed,
                end: landed,
            });
        }
        if landed == from {
            return self.answer(false);
        }
        self.emit(Effect::Restyle);
        self.emit(Effect::Trace { step: "move" });
        self.answer(true)
    }

    /// Table cell navigation keys (`page::keys::cell_move`).
    fn cell_command<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        block: usize,
        row: usize,
        column: usize,
        typing: &Typing,
    ) -> Answer {
        let Some(Some(Shape::Table { rows, columns })) = pages.read(|page| shape(page, block))
        else {
            return self.answer(false);
        };
        let Some(next) = cell_move(command, row, column, rows, columns) else {
            return self.answer(false);
        };
        self.store(pages, typing.text);
        match next {
            CellMove::To {
                row: to_row,
                column: to_column,
                add_row,
            } => {
                if add_row {
                    change(pages, |page| page.add_table_row(block, row));
                }
                self.emit(Effect::Relayout);
                let exists = pages
                    .read(|page| matches!(shape(page, block), Some(Shape::Table { rows, columns }) if to_row < rows && to_column < columns))
                    .unwrap_or(false);
                if exists {
                    self.begin(
                        pages,
                        Field::Cell {
                            block,
                            row: to_row,
                            column: to_column,
                        },
                        Caret::End,
                    );
                }
            }
            CellMove::Leave => self.emit(Effect::Relayout),
        }
        self.answer(true)
    }

    /// The `/` menu's keys while it is open and shows blocks. Returns whether it handled the
    /// command.
    fn slash_command<P: Pages>(
        &mut self,
        pages: &mut P,
        command: Command,
        typing: &Typing,
    ) -> bool {
        let Some((shown, lit)) = self
            .slash
            .as_ref()
            .map(|slash| (slash.shown.clone(), slash.lit))
        else {
            return false;
        };
        let (Some(key), false) = (slash_key(command), shown.is_empty()) else {
            return false;
        };
        let rows = vec![
            Row {
                selectable: true,
                has_submenu: false
            };
            shown.len()
        ];
        match key_intent(key, &rows, Some(lit), 0) {
            Intent::Light(lit) => {
                self.light_slash(lit);
                self.emit(Effect::Slash);
            }
            Intent::Choose(lit) => {
                let chosen = shown[lit.min(shown.len() - 1)];
                self.pick_slash(pages, chosen, typing);
            }
            Intent::CloseAll => self.close_slash(),
            Intent::Nothing | Intent::OpenSubmenu(_) | Intent::CloseSubmenu => {}
        }
        true
    }

    /// Handles choosing `/` menu entry `command` (an index into `page::edit::COMMANDS`): removes
    /// the slash and query from the line and converts the block to that kind.
    pub fn choose_slash<P: Pages>(
        &mut self,
        pages: &mut P,
        command: usize,
        typing: &Typing,
    ) -> Answer {
        let taken = self.pick_slash(pages, command, typing);
        self.answer(taken)
    }

    fn pick_slash<P: Pages>(&mut self, pages: &mut P, command: usize, typing: &Typing) -> bool {
        let Some(slash) = self.slash.take() else {
            return false;
        };
        self.emit(Effect::Slash);
        let mut text = typing.text.to_owned();
        if self.editing == Some(Field::Block(slash.block)) {
            let units = utf16(&text);
            let end = units.len().min(typing.start);
            if slash.start <= end {
                text = String::from_utf16_lossy(&[&units[..slash.start], &units[end..]].concat());
            }
        }
        self.store(pages, &text);
        // The index comes from the shell; one past the menu lands nowhere.
        let landing = match COMMANDS.get(command) {
            Some(entry) => step(pages, |page| page.set_kind(slash.block, entry.kind)),
            None => None,
        };
        self.land(pages, landing);
        true
    }

    /// Highlights `/` menu entry `lit` from the pointer or keys, scrolling it into view if it was
    /// not already highlighted.
    pub fn light_slash(&mut self, lit: usize) {
        if let Some(slash) = self.slash.as_mut().filter(|slash| lit < slash.shown.len()) {
            if slash.lit != lit {
                slash.offset = crate::menus::slash::reveal(slash.offset, lit, slash.shown.len());
            }
            slash.lit = lit;
        }
    }

    /// Scrolls the `/` menu by wheel travel `delta` (positive is down) in `unit`.
    pub fn scroll_slash(&mut self, delta: f64, unit: u32) {
        if let Some(slash) = self.slash.as_mut() {
            slash.offset =
                crate::menus::slash::scroll(slash.offset, delta, unit, slash.shown.len());
        }
    }

    // Typing -----------------------------------------------------------------------------------

    /// `replacement` is about to replace `start..end` of the field (typed, deleted or pasted). A
    /// caption joins pasted lines into one; multi-line pastes into a block become blocks; typing
    /// into formatted text goes through the core's rules so no half mark is left behind (unless an
    /// input method is composing, `marked`). Returns whether the page applied the change, in which
    /// case the field must not.
    pub fn typed<P: Pages>(
        &mut self,
        pages: &mut P,
        start: usize,
        end: usize,
        replacement: &str,
        marked: bool,
        typing: &Typing,
    ) -> Answer {
        if matches!(self.editing, Some(Field::Caption(_))) && replacement.chars().any(is_newline) {
            // A one-line field joins pasted lines, replacing each break with a space.
            let joined = replacement
                .split(is_newline)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            self.emit(Effect::Insert {
                text: joined,
                start,
                end,
            });
            return self.answer(true);
        }
        let (true, Some(Field::Block(block))) = (self.formatted(pages), self.editing) else {
            return self.answer(false);
        };
        if replacement.contains('\n') {
            self.store(pages, typing.text);
            let landing = step(pages, |page| {
                page.paste_markdown(block, start, end, replacement)
            });
            self.land(pages, landing);
            return self.answer(true);
        }
        if marked {
            return self.answer(false);
        }
        let edit = inline::replace_range(&utf16(typing.text), (start, end), &utf16(replacement));
        let text = String::from_utf16_lossy(&edit.text);
        self.emit(Effect::Apply {
            text: text.clone(),
            caret: edit.caret,
        });
        self.line_changed(pages, block, &text, edit.caret);
        self.answer(true)
    }

    /// The field's text changed on its own (an input method, or an unformatted field): a slash
    /// opens or updates the `/` menu, and a Markdown prefix converts the line to its block.
    pub fn changed<P: Pages>(&mut self, pages: &mut P, typing: &Typing) -> Answer {
        if let (true, Some(Field::Block(block))) = (self.formatted(pages), self.editing) {
            self.line_changed(pages, block, typing.text, typing.start);
        }
        self.answer(true)
    }

    fn line_changed<P: Pages>(&mut self, pages: &mut P, block: usize, text: &str, caret: usize) {
        // A slash at the start of a line, or after a space, opens the block menu, which filters
        // on the text typed after it.
        if let Some((start, query)) = find_slash_query(text, caret) {
            self.show_slash(block, start, &query);
            return;
        }
        self.close_slash();
        // A Markdown prefix at the start of a line converts it to that block. The line is saved
        // only then, not on every keystroke.
        let takes = pages
            .read(|page| {
                page.blocks
                    .get(block)
                    .is_some_and(|found| takes_shortcut(&found.kind, text, caret))
            })
            .unwrap_or(false);
        if !takes {
            return;
        }
        pages.edit(|page| (page.set_text(block, text), ()));
        let landing = step(pages, |page| page.apply_shortcut(block, caret));
        if landing.is_some() {
            self.editing = None;
            self.emit(Effect::EndEdit);
            self.land(pages, landing);
        }
    }

    /// The field's selection changed. A caret never rests inside hidden syntax or an image, a
    /// selection includes an image whole or not at all, and a link's address shows or hides as the
    /// caret enters or leaves it.
    pub fn selection<P: Pages>(&mut self, pages: &P, typing: &Typing) -> Answer {
        if !self.formatted(pages) {
            return self.answer(false);
        }
        let raw = utf16(typing.text);
        let (start, end) = if typing.start == typing.end {
            let rested = inline::snap_caret(&raw, typing.start);
            (rested, rested)
        } else {
            inline::widen_to_images(&raw, typing.start, typing.end)
        };
        if (start, end) != (typing.start, typing.end) {
            self.emit(Effect::Place { start, end });
        }
        self.emit(Effect::Restyle);
        self.emit(Effect::Trace { step: "select" });
        self.answer(true)
    }

    /// Ends editing and saves the text.
    pub fn end_typing<P: Pages>(&mut self, pages: &mut P, typing: &Typing) -> Answer {
        self.commit(pages, typing.text);
        self.answer(true)
    }

    // For a check ------------------------------------------------------------------------------

    /// Starts editing at the end of the first block.
    pub fn edit_first<P: Pages>(&mut self, pages: &P) -> Answer {
        self.edit(pages, Some(0), Caret::End);
        self.answer(true)
    }

    /// Opens the menu of block `row`, or presses its `+`, with the pointer over it.
    pub fn gutter<P: Pages>(
        &mut self,
        pages: &mut P,
        row: usize,
        plus: bool,
        typing: &Typing,
    ) -> Answer {
        self.hovered = Some(row);
        if plus {
            self.add_block(pages, row, typing);
        } else {
            self.choose(row, row, row);
            self.open_block_menu(row);
        }
        self.answer(true)
    }

    /// Focuses media block `row` (`mediafocus`), presses one of its keys (`mediadelete`,
    /// `mediareturn`, `mediaup`, `mediadown`), or edits its caption (any other step).
    pub fn debug_media<P: Pages>(&mut self, pages: &mut P, row: usize, step: &str) -> Answer {
        let key = match step {
            "mediafocus" => {
                self.focus_media(row, true);
                return self.answer(true);
            }
            "mediadelete" => MediaKey::Remove,
            "mediareturn" => MediaKey::LineBelow,
            "mediaup" => MediaKey::Step { up: true },
            "mediadown" => MediaKey::Step { up: false },
            _ => {
                if pages.read(|page| first_field(page, Some(row)))
                    == Some(Some(Field::Caption(row)))
                {
                    self.begin(pages, Field::Caption(row), Caret::End);
                }
                return self.answer(true);
            }
        };
        self.media_key(pages, key, row);
        self.answer(true)
    }
}

/// Whether `character` is a line break as the platform's text system counts one.
fn is_newline(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r' | '\u{0B}' | '\u{0C}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

#[cfg(test)]
#[path = "editor_tests.rs"]
mod tests;
