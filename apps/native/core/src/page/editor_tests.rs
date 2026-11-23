//! Tests for `page::editor` on an in-memory page: what each press, key and typed character
//! does, checked through the effects left for the shell and the page and saves afterwards.

use super::*;
use crate::page::edit::History;
use crate::page::inline_edit::Construct;

/// A page held in memory with its undo steps, counting its saves.
struct Memory {
    page: Page,
    history: History,
    saves: usize,
    read_only: bool,
    refuse_title: bool,
}

impl Pages for Memory {
    fn read<T>(&self, read: impl FnOnce(&Page) -> T) -> Option<T> {
        Some(read(&self.page))
    }

    fn edit<T>(&mut self, edit: impl FnOnce(&mut Page) -> (bool, T)) -> Option<T> {
        if self.read_only {
            return None;
        }
        self.history.record(&self.page);
        let (changed, result) = edit(&mut self.page);
        if changed {
            self.saves += 1;
        } else {
            self.history.forget_last();
        }
        Some(result)
    }

    fn rename(&mut self, title: &str) -> bool {
        if self.refuse_title {
            return false;
        }
        title.trim().clone_into(&mut self.page.title);
        true
    }

    fn undo(&mut self, redo: bool) -> bool {
        let stepped = if redo {
            self.history.redo(&mut self.page)
        } else {
            self.history.undo(&mut self.page)
        };
        if stepped {
            self.saves += 1;
        }
        stepped
    }

    fn read_only(&self) -> bool {
        self.read_only
    }
}

fn words(text: &str) -> BlockKind {
    BlockKind::Paragraph { text: text.into() }
}

fn todo(text: &str, checked: bool) -> BlockKind {
    BlockKind::Todo {
        indent: 0,
        checked,
        text: text.into(),
    }
}

fn bullet(text: &str) -> BlockKind {
    BlockKind::Bullet {
        indent: 0,
        text: text.into(),
    }
}

fn code(text: &str) -> BlockKind {
    BlockKind::Code {
        language: None,
        text: text.into(),
    }
}

fn table(rows: &[&[&str]]) -> BlockKind {
    BlockKind::Table {
        rows: rows
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect(),
        column_widths_px: None,
    }
}

fn media(caption: &str) -> BlockKind {
    BlockKind::Media {
        asset_id: "asset".into(),
        caption: caption.into(),
    }
}

fn player() -> BlockKind {
    BlockKind::Player {
        html: r#"<video src="clip.mp4" controls></video>"#.into(),
    }
}

fn page(kinds: Vec<BlockKind>) -> Memory {
    let mut made = 0;
    let mut page = Page::from_file("Notes", "# Notes\n", "en", &mut || {
        made += 1;
        format!("b{made}")
    });
    page.blocks = kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| crate::page::Block {
            id: format!("k{index}"),
            kind,
        })
        .collect();
    page.derive();
    Memory {
        page,
        history: History::default(),
        saves: 0,
        read_only: false,
        refuse_title: false,
    }
}

fn texts(pages: &Memory) -> Vec<String> {
    (0..pages.page.blocks.len())
        .map(|block| pages.page.text(block).unwrap_or("·").to_owned())
        .collect()
}

fn typing(text: &str, start: usize, end: usize) -> Typing<'_> {
    Typing { text, start, end }
}

fn begin(block: i64, text: &str, caret: usize) -> Effect {
    Effect::Begin {
        block,
        row: -1,
        column: -1,
        caption: false,
        text: text.into(),
        caret,
    }
}

fn cell(block: i64, row: i64, column: i64, text: &str, caret: usize) -> Effect {
    Effect::Begin {
        block,
        row,
        column,
        caption: false,
        text: text.into(),
        caret,
    }
}

fn letter(typed: char) -> Key {
    Key::Character(typed)
}

const NONE: Held = Held {
    command: false,
    control: false,
    option: false,
    shift: false,
};
const COMMAND: Held = Held {
    command: true,
    ..NONE
};
const SHIFT: Held = Held {
    shift: true,
    ..NONE
};
const COMMAND_SHIFT: Held = Held {
    command: true,
    shift: true,
    ..NONE
};
const COMMAND_OPTION: Held = Held {
    command: true,
    option: true,
    ..NONE
};

/// An editor typing in `field` of `pages`, with the field's text as the page has it.
fn typing_in(pages: &Memory, field: Field) -> Editor {
    let mut editor = Editor::new();
    editor.begin(pages, field, Caret::End);
    editor.answer(true);
    editor
}

/// Where blocks dragged over block `over` land, as `page::layout::drop_at` reports it: nowhere
/// when over the dragged blocks themselves.
fn over(over: Option<(usize, bool)>) -> impl FnOnce(usize, usize) -> Option<(usize, bool)> {
    move |first, last| over.filter(|&(block, _)| block < first || block > last)
}

fn chosen(editor: &Editor) -> Option<(usize, usize, usize)> {
    editor.chosen_range()
}

// The pointer ------------------------------------------------------------------------------------

#[test]
fn a_press_on_the_plus_adds_a_line_below_with_the_slash_menu_open() {
    let mut pages = page(vec![words("One"), words("Two")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    let hit = Hit {
        row: Some(0),
        button: Button::Plus,
        ..Hit::default()
    };
    let answer = editor.press(&mut pages, hit, false, &typing("Two!", 4, 4));
    assert_eq!(
        answer.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" },
            Effect::Relayout,
            begin(1, "", 0),
            Effect::Retype {
                text: "/".into(),
                caret: 1
            },
            Effect::Slash,
        ]
    );
    assert_eq!(
        texts(&pages),
        ["One", "", "Two!"],
        "what was typed is kept, and the new line is under the block"
    );
    assert_eq!(editor.editing, Some(Field::Block(1)));
    let slash = editor.slash.clone().expect("the menu is open");
    assert_eq!(
        (slash.block, slash.start, slash.shown.len(), slash.lit),
        (1, 0, COMMANDS.len(), 0)
    );
    assert_eq!((editor.hovered, editor.button), (Some(0), Button::Plus));
}

#[test]
fn a_handles_press_becomes_a_drag_as_chromiums_does_and_drops_beside_the_block_it_is_over() {
    let mut pages = page(vec![words("A"), words("B"), words("C"), words("D")]);
    let mut editor = typing_in(&pages, Field::Block(3));
    let hit = Hit {
        row: Some(0),
        button: Button::Handle,
        block: Some(0),
        ..Hit::default()
    };
    assert_eq!(
        editor
            .press(&mut pages, hit, false, &typing("D", 1, 1))
            .effects,
        [Effect::Drag]
    );
    // 1 pt at 2 px per point is not a drag yet, so typing continues.
    assert!(
        !editor
            .handle_move(
                &mut pages,
                1.0,
                0.0,
                2.0,
                over(Some((2, true))),
                &typing("D", 1, 1)
            )
            .taken
    );
    assert_eq!(editor.editing, Some(Field::Block(3)));
    let moved = editor.handle_move(
        &mut pages,
        0.0,
        1.5,
        2.0,
        over(Some((2, true))),
        &typing("D", 1, 1),
    );
    assert!(moved.taken);
    assert_eq!(
        moved.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" }
        ],
        "the drag ends typing"
    );
    assert_eq!(editor.drop, Some((2, true)));
    // Over the block it carries, it lands nowhere.
    editor.handle_move(
        &mut pages,
        0.0,
        20.0,
        2.0,
        over(Some((0, false))),
        &Typing::NONE,
    );
    assert_eq!(editor.drop, None);
    editor.handle_move(
        &mut pages,
        0.0,
        40.0,
        2.0,
        over(Some((2, true))),
        &Typing::NONE,
    );
    let dropped = editor.handle_up(&mut pages, false, &Typing::NONE);
    assert!(dropped.taken);
    assert_eq!(dropped.effects, [Effect::Relayout]);
    assert_eq!(texts(&pages), ["B", "C", "A", "D"]);
    assert_eq!((editor.drop, editor.chosen.is_none()), (None, true));
}

#[test]
fn a_drag_of_a_selected_blocks_handle_carries_the_whole_selection() {
    let mut pages = page(vec![words("A"), words("B"), words("C"), words("D")]);
    let mut editor = Editor::new();
    editor.choose(0, 1, 0);
    editor.answer(true);
    let hit = Hit {
        row: Some(1),
        button: Button::Handle,
        block: Some(1),
        ..Hit::default()
    };
    editor.press(&mut pages, hit, false, &Typing::NONE);
    editor.handle_move(
        &mut pages,
        3.0,
        0.0,
        1.0,
        over(Some((3, true))),
        &Typing::NONE,
    );
    editor.handle_up(&mut pages, false, &Typing::NONE);
    assert_eq!(texts(&pages), ["C", "D", "A", "B"]);
}

#[test]
fn a_drag_that_ends_over_nothing_moves_nothing() {
    let mut pages = page(vec![words("A"), words("B")]);
    let mut editor = Editor::new();
    editor.press(
        &mut pages,
        Hit {
            row: Some(0),
            button: Button::Handle,
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    editor.handle_move(&mut pages, 5.0, 5.0, 2.0, over(None), &Typing::NONE);
    let ended = editor.handle_up(&mut pages, false, &Typing::NONE);
    assert!(ended.taken && ended.effects.is_empty());
    assert_eq!(
        (texts(&pages), pages.saves),
        (vec!["A".to_owned(), "B".to_owned()], 0)
    );
}

#[test]
fn a_click_on_a_handle_selects_its_block_and_opens_its_menu_and_a_shift_click_grows_to_it() {
    let mut pages = page(vec![words("A"), words("B"), words("C"), words("D")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    editor.press(
        &mut pages,
        Hit {
            row: Some(2),
            button: Button::Handle,
            ..Hit::default()
        },
        false,
        &typing("B", 1, 1),
    );
    let clicked = editor.handle_up(&mut pages, false, &typing("B", 1, 1));
    assert!(!clicked.taken);
    assert_eq!(
        clicked.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" },
            Effect::TakeKeys,
            Effect::BlockMenu { block: 2 }
        ]
    );
    assert_eq!(chosen(&editor), Some((2, 2, 2)));
    assert_eq!(editor.menu_block, Some(2));
    // A shift-click on another handle grows from the anchor.
    editor.press(
        &mut pages,
        Hit {
            row: Some(0),
            button: Button::Handle,
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    editor.handle_up(&mut pages, true, &Typing::NONE);
    assert_eq!(chosen(&editor), Some((0, 2, 2)));
    // With nothing selected, a shift-click grows from the block typed in.
    let mut editor = typing_in(&pages, Field::Block(3));
    editor.press(
        &mut pages,
        Hit {
            row: Some(1),
            button: Button::Handle,
            ..Hit::default()
        },
        false,
        &typing("D", 0, 0),
    );
    editor.handle_up(&mut pages, true, &typing("D", 0, 0));
    assert_eq!((chosen(&editor), editor.editing), (Some((1, 3, 3)), None));
}

#[test]
fn a_shift_press_selects_from_the_block_typed_in_or_the_anchor_to_the_one_pressed() {
    let mut pages = page(vec![words("A"), words("B"), words("C"), words("D")]);
    let mut editor = typing_in(&pages, Field::Block(3));
    let answer = editor.press(
        &mut pages,
        Hit {
            block: Some(1),
            ..Hit::default()
        },
        true,
        &typing("Dee", 3, 3),
    );
    assert_eq!(
        answer.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" },
            Effect::TakeKeys
        ]
    );
    assert_eq!(
        (chosen(&editor), texts(&pages)[3].as_str()),
        (Some((1, 3, 3)), "Dee")
    );
    editor.press(
        &mut pages,
        Hit {
            block: Some(0),
            ..Hit::default()
        },
        true,
        &Typing::NONE,
    );
    assert_eq!(chosen(&editor), Some((0, 3, 3)), "it grows from the anchor");
    // Typing in the title, a shift-press selects the block pressed alone.
    let mut editor = typing_in(&pages, Field::Title);
    editor.press(
        &mut pages,
        Hit {
            block: Some(2),
            ..Hit::default()
        },
        true,
        &typing("Notes", 5, 5),
    );
    assert_eq!(chosen(&editor), Some((2, 2, 2)));
}

#[test]
fn a_press_selects_a_player_focuses_media_ticks_a_todo_or_types_in_a_slot() {
    let mut pages = page(vec![words("A"), player(), media("Cap"), todo("Do", false)]);
    let mut editor = Editor::new();
    let answer = editor.press(
        &mut pages,
        Hit {
            block: Some(1),
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert_eq!(
        (answer.effects, chosen(&editor)),
        (vec![Effect::TakeKeys], Some((1, 1, 1)))
    );
    // Media takes the keys without its ring: it was reached by the pointer.
    let answer = editor.press(
        &mut pages,
        Hit {
            block: Some(2),
            media: Some(2),
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert_eq!(
        (answer.effects, editor.media, editor.chosen.clone()),
        (vec![Effect::TakeKeys], Some((2, false)), None)
    );
    let answer = editor.press(
        &mut pages,
        Hit {
            block: Some(3),
            todo: Some(3),
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert_eq!(answer.effects, [Effect::Relayout]);
    assert!(matches!(
        pages.page.blocks[3].kind,
        BlockKind::Todo { checked: true, .. }
    ));
    assert_eq!((pages.saves, editor.media), (1, None));
    let answer = editor.press(
        &mut pages,
        Hit {
            block: Some(0),
            slot: Some(Field::Block(0)),
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert_eq!(answer.effects, [begin(0, "A", 1), Effect::Forward]);
    assert_eq!(editor.editing, Some(Field::Block(0)));
}

#[test]
fn a_press_below_the_last_block_types_in_an_empty_last_line() {
    let mut pages = page(vec![words("A")]);
    let mut editor = Editor::new();
    let answer = editor.press(
        &mut pages,
        Hit {
            below: true,
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert_eq!(answer.effects, [Effect::Relayout, begin(1, "", 0)]);
    assert_eq!(
        (texts(&pages), pages.saves),
        (vec!["A".to_owned(), String::new()], 0),
        "an empty line is no change to save"
    );
    // A page that already ends in one types there.
    let answer = editor.press(
        &mut pages,
        Hit {
            below: true,
            ..Hit::default()
        },
        false,
        &typing("", 0, 0),
    );
    assert_eq!(answer.effects.last(), Some(&begin(1, "", 0)));
    assert_eq!(pages.page.blocks.len(), 2);
}

#[test]
fn a_page_that_takes_no_typing_takes_no_press_and_shows_no_gutter() {
    let mut pages = page(vec![words("A")]);
    pages.read_only = true;
    let mut editor = Editor::new();
    let answer = editor.press(
        &mut pages,
        Hit {
            row: Some(0),
            button: Button::Plus,
            slot: Some(Field::Block(0)),
            ..Hit::default()
        },
        false,
        &Typing::NONE,
    );
    assert!(!answer.taken && answer.effects.is_empty());
    assert!(!editor.hover(&pages, Some(0), Button::Handle));
    assert_eq!((editor.hovered, editor.button), (None, Button::None));
    assert_eq!(editor.edit_first(&pages).effects, [Effect::Relayout]);
}

#[test]
fn hovering_says_when_the_gutter_changed() {
    let pages = page(vec![words("A"), words("B")]);
    let mut editor = Editor::new();
    assert!(editor.hover(&pages, Some(1), Button::None));
    assert!(!editor.hover(&pages, Some(1), Button::None));
    assert!(editor.hover(&pages, Some(1), Button::Handle));
    assert!(editor.hover(&pages, None, Button::None));
}

// The block menu and the table cell menu ---------------------------------------------------------

#[test]
fn the_block_menu_copies_deletes_and_turns_the_block_it_was_opened_on() {
    let mut pages = page(vec![words("A"), words("B")]);
    let mut editor = Editor::new();
    editor.gutter(&mut pages, 0, false, &Typing::NONE);
    let answer = editor.block_menu(&mut pages, PageBlockCommand::Duplicate);
    assert_eq!(answer.effects, [Effect::Relayout, Effect::TakeKeys]);
    assert_eq!(
        (texts(&pages), chosen(&editor)),
        (
            vec!["A".to_owned(), "A".to_owned(), "B".to_owned()],
            Some((1, 1, 1))
        )
    );
    let answer = editor.block_menu(&mut pages, PageBlockCommand::Turn(1));
    assert_eq!(answer.effects, [Effect::Relayout, begin(0, "A", 1)]);
    assert!(matches!(
        pages.page.blocks[0].kind,
        BlockKind::Heading { level: 1, .. }
    ));
    assert_eq!(editor.chosen, None);
    editor.block_menu(&mut pages, PageBlockCommand::Delete);
    assert_eq!(texts(&pages), ["A", "B"]);
    assert!(
        Editor::new()
            .block_menu(&mut pages, PageBlockCommand::Delete)
            .effects
            .is_empty(),
        "no menu was opened"
    );
}

#[test]
fn the_cell_menu_adds_and_takes_away_rows_and_columns_of_the_cell_it_was_opened_on() {
    let mut pages = page(vec![table(&[&["h1", "h2"], &["a", "b"]])]);
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 0,
            row: 1,
            column: 1,
        },
    );
    let opened = editor.open_table_menu(&mut pages, 0, 1, 1, &typing("bee", 3, 3));
    assert_eq!(
        opened.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" }
        ]
    );
    let rows = |pages: &Memory| match &pages.page.blocks[0].kind {
        BlockKind::Table { rows, .. } => rows.clone(),
        _ => unreachable!("a table"),
    };
    assert_eq!(rows(&pages)[1], ["a", "bee"]);
    assert_eq!(
        editor
            .table_menu(&mut pages, PageTableCommand::AddRow)
            .effects,
        [Effect::Relayout]
    );
    assert_eq!(rows(&pages).len(), 3);
    editor.table_menu(&mut pages, PageTableCommand::AddColumn);
    assert_eq!(rows(&pages)[0].len(), 3);
    editor.table_menu(&mut pages, PageTableCommand::DeleteColumn);
    assert_eq!(rows(&pages)[0], ["h1", ""]);
    editor.table_menu(&mut pages, PageTableCommand::DeleteRow);
    assert_eq!(rows(&pages).len(), 2);
}

// Keys on the page itself ----------------------------------------------------------------------

#[test]
fn backspace_deletes_the_selected_blocks_and_selects_the_block_that_takes_their_place() {
    let mut pages = page(vec![words("A"), words("B"), words("C")]);
    let mut editor = Editor::new();
    editor.choose(0, 1, 0);
    editor.answer(true);
    let answer = editor.key(&mut pages, Key::Backspace, NONE);
    assert_eq!(answer.effects, [Effect::Relayout, Effect::TakeKeys]);
    assert_eq!(
        (texts(&pages), chosen(&editor)),
        (vec!["C".to_owned()], Some((0, 0, 0)))
    );
    editor.key(&mut pages, Key::Delete, NONE);
    assert_eq!(texts(&pages), [""], "a page keeps one line");
}

#[test]
fn command_d_copies_the_selection_below_and_selects_the_copy() {
    let mut pages = page(vec![words("A"), words("B"), words("C")]);
    let mut editor = Editor::new();
    editor.choose(0, 1, 0);
    editor.answer(true);
    editor.key(&mut pages, letter('d'), COMMAND);
    assert_eq!(texts(&pages), ["A", "B", "A", "B", "C"]);
    assert_eq!(chosen(&editor), Some((2, 3, 2)));
}

#[test]
fn shift_command_arrows_move_the_selection_and_stop_at_the_edge() {
    let mut pages = page(vec![words("A"), words("B"), words("C")]);
    let mut editor = Editor::new();
    editor.choose(1, 2, 1);
    editor.answer(true);
    let answer = editor.key(&mut pages, Key::Up, COMMAND_SHIFT);
    assert_eq!(answer.effects, [Effect::Relayout, Effect::TakeKeys]);
    assert_eq!(
        (texts(&pages), chosen(&editor)),
        (
            vec!["B".to_owned(), "C".to_owned(), "A".to_owned()],
            Some((0, 1, 0))
        )
    );
    // At the top it lays out and stays selected as it was.
    assert_eq!(
        editor.key(&mut pages, Key::Up, COMMAND_SHIFT).effects,
        [Effect::Relayout]
    );
    assert_eq!(chosen(&editor), Some((0, 1, 0)));
    editor.key(&mut pages, Key::Down, COMMAND_SHIFT);
    assert_eq!(texts(&pages), ["A", "B", "C"]);
}

#[test]
fn arrows_select_the_next_block_alone_and_shift_arrows_grow_from_the_anchor() {
    let mut pages = page(vec![words("A"), words("B"), words("C"), words("D")]);
    let mut editor = Editor::new();
    editor.choose(1, 2, 1);
    editor.answer(true);
    editor.key(&mut pages, Key::Down, NONE);
    assert_eq!(chosen(&editor), Some((3, 3, 3)));
    editor.key(&mut pages, Key::Down, NONE);
    assert_eq!(
        chosen(&editor),
        Some((3, 3, 3)),
        "it stops at the last block"
    );
    editor.key(&mut pages, Key::Up, SHIFT);
    editor.key(&mut pages, Key::Up, SHIFT);
    assert_eq!(chosen(&editor), Some((1, 3, 3)));
    editor.key(&mut pages, Key::Down, SHIFT);
    assert_eq!(
        chosen(&editor),
        Some((2, 3, 3)),
        "the end that is not the anchor moves back"
    );
    editor.key(&mut pages, Key::Up, NONE);
    assert_eq!(chosen(&editor), Some((1, 1, 1)));
    editor.choose(0, 0, 0);
    editor.answer(true);
    editor.key(&mut pages, Key::Up, NONE);
    assert_eq!(
        chosen(&editor),
        Some((0, 0, 0)),
        "it stops at the first block"
    );
    editor.key(&mut pages, Key::Down, SHIFT);
    assert_eq!(chosen(&editor), Some((0, 1, 0)));
    assert_eq!(pages.saves, 0);
}

#[test]
fn return_types_at_the_end_of_the_first_selected_block_or_below_a_player() {
    let mut pages = page(vec![words("Hello"), player(), words("C")]);
    let mut editor = Editor::new();
    editor.choose(0, 1, 0);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, Key::Return, NONE).effects,
        [Effect::Relayout, begin(0, "Hello", 5)]
    );
    assert_eq!(editor.chosen, None);
    editor.choose(1, 1, 1);
    editor.answer(true);
    let answer = editor.key(&mut pages, Key::Return, NONE);
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(2, "", 0)]
            .into_iter()
            .skip(1)
            .collect::<Vec<_>>()
    );
    assert_eq!(texts(&pages), ["Hello", "·", "", "C"]);
}

#[test]
fn escape_lets_go_and_the_selection_copies_cuts_and_pastes_as_markdown() {
    let mut pages = page(vec![words("A"), bullet("B"), words("C")]);
    let mut editor = Editor::new();
    editor.choose(0, 1, 0);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, letter('c'), COMMAND).effects,
        [Effect::Copy {
            text: "A\n\n- B".into()
        }]
    );
    assert_eq!(
        editor.key(&mut pages, letter('v'), COMMAND).effects,
        [Effect::Paste]
    );
    let pasted = editor.paste_blocks(&mut pages, "X\n\nY");
    assert_eq!(pasted.effects, [Effect::Relayout, Effect::TakeKeys]);
    assert_eq!(
        (texts(&pages), chosen(&editor)),
        (
            vec!["A", "B", "X", "Y", "C"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            Some((2, 3, 2))
        )
    );
    let cut = editor.key(&mut pages, letter('x'), COMMAND);
    assert_eq!(
        cut.effects,
        [
            Effect::Copy {
                text: "X\n\nY".into()
            },
            Effect::Relayout
        ]
    );
    assert_eq!(
        (texts(&pages), editor.chosen.clone()),
        (
            vec!["A", "B", "C"]
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            None
        )
    );
    assert_eq!(
        editor.key(&mut pages, letter('a'), COMMAND).effects,
        [Effect::Pass],
        "nothing is selected to grow"
    );
    editor.choose(1, 1, 1);
    editor.answer(true);
    editor.key(&mut pages, letter('a'), COMMAND);
    assert_eq!(chosen(&editor), Some((0, 2, 0)));
    assert!(editor.key(&mut pages, Key::Escape, NONE).effects.is_empty());
    assert_eq!(editor.chosen, None);
}

#[test]
fn a_key_the_selection_does_not_take_goes_on() {
    let mut pages = page(vec![words("A")]);
    let mut editor = Editor::new();
    assert_eq!(
        editor.key(&mut pages, Key::Down, NONE).effects,
        [Effect::Pass],
        "nothing selected"
    );
    editor.choose(0, 0, 0);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, letter('q'), NONE).effects,
        [Effect::Pass]
    );
    assert_eq!(
        editor.key(&mut pages, Key::Left, NONE).effects,
        [Effect::Pass]
    );
}

#[test]
fn the_selected_blocks_take_their_shortcuts_before_the_menu_bar() {
    let mut pages = page(vec![words("A"), words("B")]);
    let mut editor = Editor::new();
    assert!(
        !editor.shortcut(&mut pages, letter('d'), COMMAND).taken,
        "nothing selected"
    );
    editor.choose(0, 0, 0);
    editor.answer(true);
    assert!(!editor.shortcut(&mut pages, letter('b'), COMMAND).taken);
    let copied = editor.shortcut(&mut pages, letter('d'), COMMAND);
    assert!(copied.taken);
    assert_eq!(texts(&pages), ["A", "A", "B"]);
    // ⌘ with another key held is the page's, and means nothing to it.
    let other = editor.shortcut(&mut pages, letter('d'), COMMAND_OPTION);
    assert_eq!((other.taken, other.effects), (true, vec![Effect::Pass]));
    let undone = editor.shortcut(&mut pages, letter('z'), COMMAND);
    assert_eq!(
        (undone.taken, undone.effects),
        (true, vec![Effect::Relayout])
    );
    assert_eq!(
        (texts(&pages), editor.chosen.clone()),
        (vec!["A".to_owned(), "B".to_owned()], None)
    );
    editor.choose(0, 0, 0);
    editor.answer(true);
    editor.shortcut(&mut pages, letter('z'), COMMAND_SHIFT);
    assert_eq!(texts(&pages), ["A", "A", "B"], "and forward again");
}

#[test]
fn a_media_block_with_the_keys_goes_deletes_and_starts_a_line_below() {
    let mut pages = page(vec![words("A"), media("Cap"), player(), media("Two")]);
    let mut editor = Editor::new();
    editor.focus_media(1, true);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, Key::Up, NONE).effects,
        [Effect::TakeKeys, Effect::Relayout, begin(0, "A", 1)]
            .into_iter()
            .skip(1)
            .collect::<Vec<_>>()
    );
    assert_eq!(editor.media, None);
    editor.focus_media(1, true);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, Key::Down, NONE).effects,
        [Effect::Relayout, Effect::TakeKeys],
        "a player below is selected"
    );
    assert_eq!(chosen(&editor), Some((2, 2, 2)));
    editor.chosen = None;
    editor.focus_media(3, false);
    editor.answer(true);
    assert_eq!(
        editor.key(&mut pages, Key::Down, NONE).effects,
        Vec::<Effect>::new(),
        "nothing is below the last"
    );
    assert_eq!(
        editor.key(&mut pages, Key::Down, SHIFT).effects,
        [Effect::Pass]
    );
    editor.key(&mut pages, Key::Return, NONE);
    assert_eq!(texts(&pages), ["A", "·", "·", "·", ""]);
    assert_eq!(editor.editing, Some(Field::Block(4)));
    editor.focus_media(1, true);
    editor.answer(true);
    let removed = editor.key(&mut pages, Key::Backspace, NONE);
    assert_eq!(removed.effects[0], Effect::Relayout);
    assert_eq!(pages.page.blocks.len(), 4);
    assert!(matches!(
        pages.page.blocks[1].kind,
        BlockKind::Player { .. }
    ));
}

// Shortcuts while a field is typed in -----------------------------------------------------------

#[test]
fn formatting_shortcuts_mark_the_selected_words_and_keep_them_selected() {
    let mut pages = page(vec![words("make bold")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer =
        editor.field_shortcut(&mut pages, letter('b'), COMMAND, &typing("make bold", 5, 9));
    assert!(answer.taken);
    assert_eq!(
        answer.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            begin(0, "make **bold**", 5),
            Effect::Select { start: 5, end: 13 }
        ]
    );
    assert_eq!(texts(&pages), ["make **bold**"]);
    let struck = editor.field_shortcut(
        &mut pages,
        letter('s'),
        COMMAND_SHIFT,
        &typing("make **bold**", 0, 4),
    );
    assert_eq!(texts(&pages), ["~~make~~ **bold**"]);
    assert_eq!(
        struck.effects.last(),
        Some(&Effect::Select { start: 0, end: 8 })
    );
    for (key, mark) in [('i', "*"), ('e', "`")] {
        let mut pages = page(vec![words("x")]);
        let mut editor = typing_in(&pages, Field::Block(0));
        editor.field_shortcut(&mut pages, letter(key), COMMAND, &typing("x", 0, 1));
        assert_eq!(texts(&pages), [format!("{mark}x{mark}")]);
    }
    let _ = Construct::Bold;
}

#[test]
fn command_k_starts_a_link_and_command_option_digits_turn_the_line() {
    let mut pages = page(vec![words("see")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let linked = editor.field_shortcut(&mut pages, letter('k'), COMMAND, &typing("see", 0, 3));
    assert_eq!(texts(&pages), ["[see]()"]);
    assert_eq!(
        linked.effects.last(),
        Some(&begin(0, "[see]()", 6)),
        "the caret goes where the address is typed"
    );
    let turned = editor.field_shortcut(
        &mut pages,
        letter('4'),
        COMMAND_OPTION,
        &typing("[see]()", 0, 0),
    );
    assert!(matches!(pages.page.blocks[0].kind, BlockKind::Todo { .. }));
    assert_eq!(turned.effects.last(), Some(&begin(0, "[see]()", 7)));
    assert!(
        !editor
            .field_shortcut(
                &mut pages,
                letter('7'),
                COMMAND_OPTION,
                &typing("[see]()", 0, 0)
            )
            .taken
    );
}

#[test]
fn code_cells_captions_and_the_title_take_no_formatting() {
    let mut pages = page(vec![code("let x"), table(&[&["a"]]), media("Cap")]);
    for field in [
        Field::Block(0),
        Field::Cell {
            block: 1,
            row: 0,
            column: 0,
        },
        Field::Caption(2),
        Field::Title,
    ] {
        let mut editor = typing_in(&pages, field);
        for (key, held) in [
            (letter('b'), COMMAND),
            (letter('k'), COMMAND),
            (letter('1'), COMMAND_OPTION),
        ] {
            let answer = editor.field_shortcut(&mut pages, key, held, &typing("x", 0, 1));
            assert!(
                !answer.taken && answer.effects.is_empty(),
                "{field:?} {key:?}"
            );
        }
    }
    // A caption and the title take none of the block's own shortcuts either; code and a cell do.
    for field in [Field::Caption(2), Field::Title] {
        let mut editor = typing_in(&pages, field);
        assert!(
            !editor
                .field_shortcut(&mut pages, letter('d'), COMMAND, &typing("x", 0, 0))
                .taken
        );
        assert!(
            !editor
                .field_shortcut(&mut pages, Key::Return, COMMAND, &typing("x", 0, 0))
                .taken
        );
    }
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 1,
            row: 0,
            column: 0,
        },
    );
    let copied = editor.field_shortcut(&mut pages, letter('d'), COMMAND, &typing("a", 1, 1));
    assert!(copied.taken);
    assert_eq!(
        copied.effects.last(),
        Some(&cell(2, 0, 0, "a", 1)),
        "the copy's first cell"
    );
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.field_shortcut(&mut pages, Key::Down, COMMAND_SHIFT, &typing("let x", 2, 2));
    assert!(
        matches!(pages.page.blocks[1].kind, BlockKind::Code { .. }),
        "code moved down"
    );
}

#[test]
fn undo_steps_back_across_blocks_and_types_on_in_the_same_field() {
    let mut pages = page(vec![words("A"), table(&[&["a", "b"]]), media("Cap")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.field_shortcut(&mut pages, letter('d'), COMMAND, &typing("A", 1, 1));
    assert_eq!(texts(&pages).len(), 4);
    let undone = editor.field_shortcut(&mut pages, letter('z'), COMMAND, &typing("A", 1, 1));
    // The copy was typed in; once it is gone its place is the table's, whose first cell types on.
    assert_eq!(
        undone.effects,
        [Effect::EndEdit, Effect::Relayout, cell(1, 0, 0, "a", 1)]
    );
    assert_eq!(texts(&pages).len(), 3);
    // A caption takes undo too, and types on in its own field; a cell in its table's first.
    let mut editor = typing_in(&pages, Field::Caption(2));
    let answer = editor.field_shortcut(
        &mut pages,
        letter('z'),
        COMMAND_SHIFT,
        &typing("Cap!", 4, 4),
    );
    assert_eq!(
        answer.effects.last(),
        Some(&Effect::Begin {
            block: 2,
            row: -1,
            column: -1,
            caption: true,
            text: "Cap!".into(),
            caret: 4
        })
    );
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 1,
            row: 0,
            column: 1,
        },
    );
    let answer = editor.field_shortcut(&mut pages, letter('z'), COMMAND, &typing("b", 1, 1));
    assert_eq!(answer.effects.last(), Some(&cell(1, 0, 0, "a", 1)));
    // With its block gone, the page's last field.
    let mut pages = page(vec![words("A")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.blocks_key(&mut pages, BlocksKey::Clear);
    editor.field_shortcut(&mut pages, letter('d'), COMMAND, &typing("A", 0, 0));
    let mut editor = typing_in(&pages, Field::Block(1));
    let answer = editor.field_shortcut(&mut pages, letter('z'), COMMAND, &typing("A", 0, 0));
    assert_eq!(answer.effects.last(), Some(&begin(0, "A", 1)));
}

#[test]
fn command_return_ticks_a_todo_and_shift_command_arrows_move_the_block() {
    let mut pages = page(vec![todo("Do", false), words("B")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let ticked = editor.field_shortcut(&mut pages, Key::Return, COMMAND, &typing("Do it", 2, 2));
    assert_eq!(
        ticked.effects,
        [Effect::EndEdit, Effect::Relayout, begin(0, "Do it", 2)]
    );
    assert!(
        matches!(&pages.page.blocks[0].kind, BlockKind::Todo { checked: true, text, .. } if text == "Do it")
    );
    let moved = editor.field_shortcut(&mut pages, Key::Down, COMMAND_SHIFT, &typing("Do it", 2, 2));
    assert_eq!(moved.effects.last(), Some(&begin(1, "Do it", 2)));
    assert_eq!(texts(&pages), ["B", "Do it"]);
    let stuck = editor.field_shortcut(&mut pages, Key::Down, COMMAND_SHIFT, &typing("Do it", 2, 2));
    assert_eq!(
        stuck.effects.last(),
        Some(&begin(1, "Do it", 2)),
        "at the bottom it stays"
    );
    assert!(
        !editor
            .field_shortcut(&mut pages, letter('q'), COMMAND, &typing("Do it", 2, 2))
            .taken
    );
}

// The field's commands ---------------------------------------------------------------------------

#[test]
fn a_captions_return_tab_and_escape_do_nothing_to_the_page() {
    let mut pages = page(vec![media("Cap")]);
    let mut editor = typing_in(&pages, Field::Caption(0));
    for command in [
        Command::Newline,
        Command::Tab,
        Command::Backtab,
        Command::Cancel,
    ] {
        let answer = editor.command(&mut pages, command, &typing("Cap", 3, 3));
        assert!(answer.taken && answer.effects.is_empty());
    }
    for command in [Command::Left, Command::DeleteBackward, Command::Up] {
        assert!(
            !editor
                .command(&mut pages, command, &typing("Cap", 3, 3))
                .taken
        );
    }
    assert_eq!(editor.editing, Some(Field::Caption(0)));
}

#[test]
fn escape_keeps_the_words_and_selects_their_block() {
    let mut pages = page(vec![words("A"), table(&[&["a"]])]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.command(&mut pages, Command::Cancel, &typing("Ab", 2, 2));
    assert_eq!(
        answer.effects,
        [
            Effect::EndEdit,
            Effect::Relayout,
            Effect::Trace { step: "commit" },
            Effect::TakeKeys
        ]
    );
    assert_eq!(
        (texts(&pages)[0].as_str(), chosen(&editor)),
        ("Ab", Some((0, 0, 0)))
    );
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 1,
            row: 0,
            column: 0,
        },
    );
    editor.command(&mut pages, Command::Cancel, &typing("a", 1, 1));
    assert_eq!(chosen(&editor), Some((1, 1, 1)), "a cell selects its table");
    let mut editor = typing_in(&pages, Field::Title);
    editor.command(&mut pages, Command::Cancel, &typing("Title", 5, 5));
    assert_eq!(
        (editor.chosen.clone(), pages.page.title.as_str()),
        (None, "Title")
    );
}

#[test]
fn the_titles_return_goes_to_the_first_block_and_a_refused_title_beeps() {
    let mut pages = page(vec![words("A")]);
    let mut editor = typing_in(&pages, Field::Title);
    let answer = editor.command(&mut pages, Command::Newline, &typing("Plans", 5, 5));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(0, "A", 0)]
    );
    assert_eq!(pages.page.title, "Plans");
    let mut editor = typing_in(&pages, Field::Title);
    assert!(
        !editor
            .command(&mut pages, Command::Tab, &typing("Plans", 5, 5))
            .taken
    );
    pages.refuse_title = true;
    let answer = editor.end_typing(&mut pages, &typing("Bad/Name", 3, 3));
    assert_eq!(
        answer.effects,
        [
            Effect::EndEdit,
            Effect::Beep,
            Effect::Relayout,
            Effect::Trace { step: "commit" }
        ]
    );
}

#[test]
fn return_splits_the_block_except_in_code() {
    let mut pages = page(vec![words("Hello world"), code("a")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.command(&mut pages, Command::Newline, &typing("Hello world", 5, 5));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(1, " world", 0)]
    );
    assert_eq!(texts(&pages), ["Hello", " world", "a"]);
    let mut editor = typing_in(&pages, Field::Block(2));
    assert!(
        !editor
            .command(&mut pages, Command::Newline, &typing("a", 1, 1))
            .taken,
        "a line of code"
    );
}

#[test]
fn left_and_right_pass_hidden_marks_and_stop_at_the_lines_edges() {
    let mut pages = page(vec![words("a **b** c")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.command(&mut pages, Command::Right, &typing("a **b** c", 2, 2));
    assert_eq!(
        answer.effects,
        [
            Effect::Place { start: 5, end: 5 },
            Effect::Restyle,
            Effect::Trace { step: "move" }
        ]
    );
    assert!(
        !editor
            .command(&mut pages, Command::Left, &typing("a **b** c", 0, 0))
            .taken
    );
    assert!(
        !editor
            .command(&mut pages, Command::Right, &typing("a **b** c", 2, 4))
            .taken,
        "a selection is the field's"
    );
}

#[test]
fn backspace_and_delete_take_one_shown_character_or_join_the_lines() {
    let mut pages = page(vec![words("ab"), words("**c**")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    let answer = editor.command(&mut pages, Command::DeleteForward, &typing("**c**", 2, 2));
    assert_eq!(
        answer.effects,
        [Effect::Apply {
            text: String::new(),
            caret: 0
        }]
    );
    let joined = editor.command(&mut pages, Command::DeleteBackward, &typing("**c**", 2, 2));
    assert_eq!(
        joined.effects,
        [Effect::EndEdit, Effect::Relayout, begin(0, "ab**c**", 2)]
    );
    assert_eq!(texts(&pages), ["ab**c**"]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.command(&mut pages, Command::DeleteForward, &typing("ab", 2, 2));
    assert_eq!(texts(&pages), ["ab"], "nothing below to join");
}

#[test]
fn code_joins_at_its_edges_and_keeps_its_tabs_and_deletions() {
    let mut pages = page(vec![words("A"), code("x"), words("B")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    assert!(
        !editor
            .command(&mut pages, Command::DeleteBackward, &typing("x", 1, 1))
            .taken
    );
    assert!(
        !editor
            .command(&mut pages, Command::Tab, &typing("x", 1, 1))
            .taken
    );
    assert!(
        !editor
            .command(&mut pages, Command::Left, &typing("x", 1, 1))
            .taken
    );
    let joined = editor.command(&mut pages, Command::DeleteForward, &typing("x", 1, 1));
    assert!(joined.taken);
    assert_eq!(texts(&pages)[1], "xB");
    let mut editor = typing_in(&pages, Field::Block(1));
    let answer = editor.command(&mut pages, Command::DeleteBackward, &typing("xB", 0, 0));
    assert!(answer.taken);
}

#[test]
fn tab_indents_a_list_item_and_stays_in_it() {
    let mut pages = page(vec![bullet("a"), bullet("b")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    let answer = editor.command(&mut pages, Command::Tab, &typing("b", 1, 1));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(1, "b", 1)]
    );
    assert!(matches!(
        pages.page.blocks[1].kind,
        BlockKind::Bullet { indent: 1, .. }
    ));
    editor.command(&mut pages, Command::Backtab, &typing("b", 0, 0));
    assert!(matches!(
        pages.page.blocks[1].kind,
        BlockKind::Bullet { indent: 0, .. }
    ));
}

#[test]
fn up_at_a_lines_start_passes_players_to_the_words_above_or_the_title() {
    let mut pages = page(vec![words("A"), player(), player(), words("B")]);
    let mut editor = typing_in(&pages, Field::Block(3));
    let answer = editor.command(&mut pages, Command::Up, &typing("B", 0, 1));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(0, "A", 1)]
    );
    let mut pages = page(vec![player(), words("B")]);
    let mut editor = typing_in(&pages, Field::Block(1));
    let answer = editor.command(&mut pages, Command::Up, &typing("B", 0, 0));
    assert_eq!(answer.effects.last(), Some(&begin(-1, "Notes", 5)));
    let mut first = page(vec![words("A")]);
    let mut editor = typing_in(&first, Field::Block(0));
    assert!(
        !editor
            .command(&mut first, Command::Up, &typing("A", 0, 0))
            .taken,
        "the first block's own"
    );
}

#[test]
fn up_and_down_onto_an_images_line_land_beside_it() {
    let line = "ab ![g](g.png)cd";
    let mut pages = page(vec![words(line)]);
    let mut editor = typing_in(&pages, Field::Block(0));
    assert_eq!(
        editor
            .command(&mut pages, Command::Down, &typing(line, 1, 1))
            .effects,
        [Effect::Vertical { up: false }]
    );
    assert_eq!(
        editor
            .command(&mut pages, Command::Up, &typing(line, 15, 15))
            .effects,
        [Effect::Vertical { up: true }]
    );
    // Where the field put the caret: inside the image, it goes to the side it came from.
    let down = editor.vertical(1, false, &typing(line, 6, 6));
    assert_eq!(
        (down.taken, down.effects),
        (
            true,
            vec![
                Effect::Place { start: 14, end: 14 },
                Effect::Restyle,
                Effect::Trace { step: "move" }
            ]
        )
    );
    let up = editor.vertical(15, true, &typing(line, 6, 6));
    assert_eq!(up.effects[0], Effect::Place { start: 3, end: 3 });
    let still = editor.vertical(1, false, &typing(line, 1, 1));
    assert!(!still.taken && still.effects.is_empty());
    let snapped_back = editor.vertical(3, true, &typing(line, 6, 6));
    assert_eq!(
        (snapped_back.taken, snapped_back.effects),
        (false, vec![Effect::Place { start: 3, end: 3 }])
    );
    // A line with no image, or the caret at its edge, is not this.
    let mut plain = page(vec![words("ab"), words("cd")]);
    let mut editor = typing_in(&plain, Field::Block(0));
    assert!(
        !editor
            .command(&mut plain, Command::Down, &typing("ab", 1, 1))
            .taken
    );
}

#[test]
fn down_at_a_lines_end_goes_to_media_the_next_slot_or_stays() {
    let mut pages = page(vec![
        words("A"),
        media("Cap"),
        words("B"),
        player(),
        BlockKind::Divider,
        table(&[&["t"]]),
    ]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.command(&mut pages, Command::Down, &typing("A", 1, 1));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, Effect::TakeKeys]
    );
    assert_eq!(editor.media, Some((1, true)));
    let mut editor = typing_in(&pages, Field::Block(2));
    let answer = editor.command(&mut pages, Command::Down, &typing("B", 1, 1));
    assert_eq!(
        answer.effects.last(),
        Some(&cell(5, 0, 0, "t", 0)),
        "past the player and the divider"
    );
    let mut last = page(vec![words("A"), words("Z")]);
    let mut editor = typing_in(&last, Field::Block(1));
    let answer = editor.command(&mut last, Command::Down, &typing("Zz", 2, 2));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(1, "Zz", 2)]
    );
}

#[test]
fn a_cells_keys_move_between_cells_adding_rows_past_the_last() {
    let mut pages = page(vec![table(&[&["a", "b"], &["c", "d"]])]);
    let rows = |pages: &Memory| match &pages.page.blocks[0].kind {
        BlockKind::Table { rows, .. } => rows.len(),
        _ => 0,
    };
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 0,
            row: 1,
            column: 0,
        },
    );
    let answer = editor.command(&mut pages, Command::Newline, &typing("cc", 2, 2));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, cell(0, 2, 0, "", 0)]
    );
    assert_eq!(rows(&pages), 3);
    let answer = editor.command(&mut pages, Command::Up, &typing("", 0, 0));
    assert_eq!(answer.effects.last(), Some(&cell(0, 1, 0, "cc", 2)));
    editor.command(&mut pages, Command::Tab, &typing("cc", 2, 2));
    assert_eq!(
        editor.editing,
        Some(Field::Cell {
            block: 0,
            row: 1,
            column: 1
        })
    );
    editor.command(&mut pages, Command::Tab, &typing("d", 1, 1));
    assert_eq!(
        editor.editing,
        Some(Field::Cell {
            block: 0,
            row: 2,
            column: 0
        })
    );
    editor.command(&mut pages, Command::Backtab, &typing("", 0, 0));
    assert_eq!(
        editor.editing,
        Some(Field::Cell {
            block: 0,
            row: 1,
            column: 1
        })
    );
    assert!(
        !editor
            .command(&mut pages, Command::Left, &typing("d", 1, 1))
            .taken
    );
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 0,
            row: 0,
            column: 0,
        },
    );
    let left = editor.command(&mut pages, Command::Backtab, &typing("a", 1, 1));
    assert_eq!(
        (left.effects, editor.editing),
        (vec![Effect::EndEdit, Effect::Relayout], None)
    );
}

// The `/` menu ---------------------------------------------------------------------------------

#[test]
fn a_slash_opens_the_menu_which_follows_the_words_typed_after_it() {
    let mut pages = page(vec![words("")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let typed = editor.typed(&mut pages, 0, 0, "/", false, &typing("", 0, 0));
    assert_eq!(
        typed.effects,
        [
            Effect::Apply {
                text: "/".into(),
                caret: 1
            },
            Effect::Slash
        ]
    );
    assert_eq!(
        editor.slash.as_ref().map(|slash| slash.shown.len()),
        Some(COMMANDS.len())
    );
    editor.command(&mut pages, Command::Down, &typing("/", 1, 1));
    editor.command(&mut pages, Command::Down, &typing("/", 1, 1));
    assert_eq!(editor.slash.as_ref().map(|slash| slash.lit), Some(2));
    editor.typed(&mut pages, 1, 1, "h", false, &typing("/", 1, 1));
    let slash = editor.slash.clone().expect("open");
    assert_eq!(
        (slash.shown, slash.lit),
        (vec![1, 2, 3, 9], 0),
        "new entries light the first"
    );
    editor.command(&mut pages, Command::Up, &typing("/h", 2, 2));
    assert_eq!(
        editor.slash.as_ref().map(|slash| slash.lit),
        Some(3),
        "↑ goes round"
    );
    editor.command(&mut pages, Command::Down, &typing("/h", 2, 2));
    editor.command(&mut pages, Command::Down, &typing("/h", 2, 2));
    let chosen = editor.command(&mut pages, Command::Tab, &typing("/h", 2, 2));
    assert_eq!(
        chosen.effects,
        [
            Effect::Slash,
            Effect::EndEdit,
            Effect::Relayout,
            begin(0, "", 0)
        ]
    );
    assert!(matches!(
        pages.page.blocks[0].kind,
        BlockKind::Heading { level: 2, .. }
    ));
    assert_eq!(editor.slash, None);
    assert_eq!(
        pages.saves, 1,
        "the slash and its query never reach the file"
    );
}

#[test]
fn the_slash_menu_keeps_its_lit_row_in_view_and_scrolls_by_the_wheel() {
    let mut pages = page(vec![words("")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.typed(&mut pages, 0, 0, "/", false, &typing("", 0, 0));
    let count = COMMANDS.len();
    let offset = |editor: &Editor| editor.slash.as_ref().map(|slash| slash.offset);
    assert_eq!(offset(&editor), Some(0.0));
    // ↑ from the first goes round to the last, which is scrolled up into view.
    editor.command(&mut pages, Command::Up, &typing("/", 1, 1));
    let revealed = crate::menus::slash::reveal(0.0, count - 1, count);
    assert!(revealed > 0.0, "the menu of every block scrolls");
    assert_eq!(offset(&editor), Some(revealed));
    // The pointer on the row lit already moves nothing; the wheel is held to the rows.
    editor.light_slash(count - 1);
    assert_eq!(offset(&editor), Some(revealed));
    editor.scroll_slash(-1000.0, crate::scroll::route::WHEEL_POINTS);
    assert_eq!(offset(&editor), Some(0.0));
    // Typed words that keep the same entries keep the scroll.
    editor.scroll_slash(20.0, crate::scroll::route::WHEEL_POINTS);
    editor.typed(&mut pages, 1, 1, "", false, &typing("/", 1, 1));
    assert_eq!(offset(&editor), Some(20.0));
}

#[test]
fn the_slash_menu_closes_on_escape_and_leaves_keys_alone_when_nothing_matches() {
    let mut pages = page(vec![words("a")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.typed(&mut pages, 1, 1, " /", false, &typing("a", 1, 1));
    assert!(editor.slash.is_some());
    let closed = editor.command(&mut pages, Command::Cancel, &typing("a /", 3, 3));
    assert_eq!((closed.taken, closed.effects), (true, vec![Effect::Slash]));
    assert_eq!(
        (editor.slash.clone(), editor.editing),
        (None, Some(Field::Block(0)))
    );
    editor.typed(&mut pages, 3, 3, "zzz", false, &typing("a /", 3, 3));
    assert_eq!(
        editor.slash.as_ref().map(|slash| slash.shown.is_empty()),
        Some(true)
    );
    assert!(
        !editor
            .command(&mut pages, Command::Up, &typing("a /zzz", 6, 6))
            .taken,
        "the line's"
    );
    // A space ends the query and closes the menu.
    editor.typed(&mut pages, 6, 6, " ", false, &typing("a /zzz", 6, 6));
    assert_eq!(editor.slash, None);
}

#[test]
fn a_slash_entry_chosen_with_the_pointer_takes_the_query_out_of_the_line() {
    let mut pages = page(vec![words("ab")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.typed(&mut pages, 2, 2, " /ta", false, &typing("ab", 2, 2));
    editor.light_slash(0);
    let chosen = editor.choose_slash(&mut pages, 10, &typing("ab /ta", 6, 6));
    assert_eq!(
        chosen.effects[..3],
        [Effect::Slash, Effect::EndEdit, Effect::Relayout]
    );
    assert_eq!(pages.page.text(0), Some("ab "));
    assert!(
        matches!(pages.page.blocks[1].kind, BlockKind::Table { .. }),
        "a table goes below a line with words"
    );
    assert_eq!(chosen.effects.last(), Some(&cell(1, 0, 0, "", 0)));
    assert!(
        !editor.choose_slash(&mut pages, 0, &Typing::NONE).taken,
        "no menu is open"
    );
}

// Typing ---------------------------------------------------------------------------------------

#[test]
fn a_caption_takes_pasted_lines_as_one() {
    let mut pages = page(vec![media("")]);
    let mut editor = typing_in(&pages, Field::Caption(0));
    let answer = editor.typed(
        &mut pages,
        0,
        0,
        "one\n\ntwo\r\nthree\n",
        false,
        &typing("", 0, 0),
    );
    assert_eq!(
        answer.effects,
        [Effect::Insert {
            text: "one two three".into(),
            start: 0,
            end: 0
        }]
    );
    assert!(
        !editor
            .typed(&mut pages, 0, 0, "x", false, &typing("", 0, 0))
            .taken,
        "a caption types as itself"
    );
    assert!(editor
        .changed(&mut pages, &typing("x", 1, 1))
        .effects
        .is_empty());
}

#[test]
fn several_lines_pasted_into_a_block_make_blocks_of_them() {
    let mut pages = page(vec![words("ab")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.typed(&mut pages, 1, 1, "x\n\ny", false, &typing("ab", 1, 1));
    assert!(answer.taken);
    assert_eq!(texts(&pages), ["a", "x", "y", "b"]);
    assert_eq!(
        answer.effects.last(),
        Some(&begin(2, "y", 1)),
        "at the end of the last line pasted"
    );
}

#[test]
fn typing_goes_through_the_cores_rules_unless_an_input_method_composes() {
    let mut pages = page(vec![words("**b**")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.typed(&mut pages, 2, 3, "", false, &typing("**b**", 2, 3));
    assert_eq!(
        answer.effects,
        [Effect::Apply {
            text: String::new(),
            caret: 0
        }],
        "no half of a mark is left"
    );
    assert!(
        !editor
            .typed(&mut pages, 2, 2, "か", true, &typing("**b**", 2, 2))
            .taken
    );
    assert_eq!(pages.saves, 0);
    // Not formatted words: the field types as itself.
    let mut pages = page(vec![code("x")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    assert!(
        !editor
            .typed(&mut pages, 1, 1, "y", false, &typing("x", 1, 1))
            .taken
    );
}

#[test]
fn a_markdown_prefix_turns_the_line_and_a_line_that_is_none_is_not_saved_as_it_is_typed() {
    let mut pages = page(vec![words("")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.typed(&mut pages, 0, 0, "-", false, &typing("", 0, 0));
    editor.typed(&mut pages, 1, 1, "x", false, &typing("-", 1, 1));
    editor.typed(&mut pages, 2, 2, "y", false, &typing("-x", 2, 2));
    assert_eq!(
        (pages.saves, texts(&pages)),
        (0, vec![String::new()]),
        "typing that is no shortcut is not saved"
    );
    let mut editor = typing_in(&pages, Field::Block(0));
    let answer = editor.typed(&mut pages, 1, 1, " ", false, &typing("#", 1, 1));
    assert_eq!(
        answer.effects,
        [
            Effect::Apply {
                text: "# ".into(),
                caret: 2
            },
            Effect::EndEdit,
            Effect::Relayout,
            begin(0, "", 0)
        ]
    );
    assert!(matches!(
        pages.page.blocks[0].kind,
        BlockKind::Heading { level: 1, .. }
    ));
    assert_eq!(
        pages.saves, 1,
        "the line is saved once it turns; its file reads the same as a heading"
    );
    let mut editor = typing_in(&pages, Field::Block(0));
    editor.changed(&mut pages, &typing("---", 3, 3));
    assert!(
        matches!(pages.page.blocks[0].kind, BlockKind::Heading { .. }),
        "a heading takes no divider"
    );
    let mut plain = page(vec![words("")]);
    let mut editor = typing_in(&plain, Field::Block(0));
    let answer = editor.changed(&mut plain, &typing("---", 3, 3));
    assert_eq!(
        answer.effects,
        [Effect::EndEdit, Effect::Relayout, begin(1, "", 0)]
    );
    assert_eq!(plain.page.blocks[0].kind, BlockKind::Divider);
}

#[test]
fn a_caret_never_rests_in_hidden_marks_and_a_selection_takes_images_whole() {
    let mut pages = page(vec![words("a **b**"), code("x")]);
    let mut editor = typing_in(&pages, Field::Block(0));
    assert_eq!(
        editor.selection(&pages, &typing("a **b**", 3, 3)).effects,
        [
            Effect::Place { start: 4, end: 4 },
            Effect::Restyle,
            Effect::Trace { step: "select" }
        ]
    );
    assert_eq!(
        editor.selection(&pages, &typing("a **b**", 1, 1)).effects,
        [Effect::Restyle, Effect::Trace { step: "select" }]
    );
    let line = "ab ![g](g.png)cd";
    assert_eq!(
        editor.selection(&pages, &typing(line, 1, 6)).effects[0],
        Effect::Place { start: 1, end: 14 }
    );
    let editor_in_code = &mut typing_in(&pages, Field::Block(1));
    assert!(
        editor_in_code
            .selection(&pages, &typing("x", 0, 0))
            .effects
            .is_empty(),
        "code is not restyled"
    );
    let _ = &mut pages;
}

#[test]
fn typing_ends_keeping_each_fields_words() {
    let mut pages = page(vec![table(&[&["a"]]), media("")]);
    let mut editor = typing_in(
        &pages,
        Field::Cell {
            block: 0,
            row: 0,
            column: 0,
        },
    );
    editor.end_typing(&mut pages, &typing("alpha", 5, 5));
    let mut editor = typing_in(&pages, Field::Caption(1));
    editor.end_typing(&mut pages, &typing("A picture", 9, 9));
    assert_eq!(pages.page.blocks[0].kind, table(&[&["alpha"]]));
    assert_eq!(pages.page.blocks[1].kind, media("A picture"));
    // With nothing typed in, ending changes nothing.
    assert!(Editor::new()
        .end_typing(&mut pages, &Typing::NONE)
        .effects
        .is_empty());
}

// For a check ----------------------------------------------------------------------------------

#[test]
fn a_checks_steps_reach_the_gutter_media_and_the_first_block() {
    let mut pages = page(vec![words("A"), media("Cap")]);
    let mut editor = Editor::new();
    assert_eq!(
        editor.edit_first(&pages).effects,
        [Effect::Relayout, begin(0, "A", 1)]
    );
    assert_eq!(
        editor
            .gutter(&mut pages, 1, false, &typing("A", 1, 1))
            .effects,
        [Effect::TakeKeys, Effect::BlockMenu { block: 1 }]
    );
    assert_eq!(editor.hovered, Some(1));
    assert_eq!(
        editor.debug_media(&mut pages, 1, "mediafocus").effects,
        [Effect::TakeKeys]
    );
    assert_eq!(editor.media, Some((1, true)));
    let caption = editor.debug_media(&mut pages, 1, "caption");
    assert_eq!(
        caption.effects,
        [Effect::Begin {
            block: 1,
            row: -1,
            column: -1,
            caption: true,
            text: "Cap".into(),
            caret: 3
        }]
    );
    editor.editing = None;
    editor.debug_media(&mut pages, 1, "mediareturn");
    assert_eq!(texts(&pages), ["A", "·", ""]);
    editor.debug_media(&mut pages, 1, "mediaup");
    assert_eq!(editor.editing, Some(Field::Block(0)));
    editor.debug_media(&mut pages, 1, "mediadelete");
    assert_eq!(pages.page.blocks.len(), 2);
}
