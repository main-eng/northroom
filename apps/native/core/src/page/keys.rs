//! What a key does on the open page: to whole selected blocks, to a focused media block, to the
//! field being typed in (its shortcuts and the text view's commands), to a table cell and to
//! the `/` menu.
//!
//! A shell names the key (`NrKeyPress`, from its key-code table; a letter is identified by its
//! US-layout position, whatever the active layout types) and which of ⌘, Control, ⌥ and ⇧ are
//! held (`row_keys::Held`). Keys the text view handles itself (arrows, deletion, Return) arrive
//! as a `Command`, named after the key that triggers it.

use crate::ffi::{NrKey, NrKeyPress};
pub use crate::list::row_keys::Held;
use crate::page::inline_edit::Construct;

/// The keys the page answers to; any other key is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The key above Return.
    Backspace,
    /// The forward delete key.
    Delete,
    Return,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Tab,
    /// A letter or digit key, by what it types on the US layout, in lower case.
    Character(char),
    Other,
}

impl Key {
    /// A key as the FFI names it: a letter or digit by its US-layout position.
    pub fn from_press(press: NrKeyPress) -> Self {
        match press.key {
            NrKey::Backspace => Self::Backspace,
            NrKey::Delete => Self::Delete,
            NrKey::Return => Self::Return,
            NrKey::Escape => Self::Escape,
            NrKey::Up => Self::Up,
            NrKey::Down => Self::Down,
            NrKey::Left => Self::Left,
            NrKey::Right => Self::Right,
            NrKey::Tab => Self::Tab,
            _ => press.letter().map_or(Self::Other, Self::Character),
        }
    }

    fn vertical(self) -> Option<bool> {
        match self {
            Self::Up => Some(true),
            Self::Down => Some(false),
            _ => None,
        }
    }
}

/// The held modifiers from the FFI bit flags (`menus::COMMAND`…).
pub fn held(bits: u32) -> Held {
    Held {
        command: bits & crate::menus::COMMAND != 0,
        control: bits & crate::menus::CONTROL != 0,
        option: bits & crate::menus::OPTION != 0,
        shift: bits & crate::menus::SHIFT != 0,
    }
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

/// What a key does to whole selected blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlocksKey {
    /// ⌫ and ⌦: delete the blocks and select the one that takes their place.
    Delete,
    /// ⌘D: duplicate them below and select the copy.
    Duplicate,
    /// ⇧⌘↑ and ⇧⌘↓: swap them with the block above or below.
    Move {
        up: bool,
    },
    /// ↑ and ↓: select only the block above the first or below the last.
    Step {
        up: bool,
    },
    /// ⇧↑ and ⇧↓: move the end opposite the anchor.
    Grow {
        up: bool,
    },
    /// Return: start typing at the end of the first, or on a new line below a player.
    Edit,
    /// Escape: clear the selection.
    Clear,
    Copy,
    Cut,
    Paste,
    SelectAll,
}

/// What `key` does to the selected blocks when exactly the modifiers it names are `held`.
pub fn blocks_key(key: Key, held: Held) -> Option<BlocksKey> {
    Some(match (key, held) {
        (Key::Backspace | Key::Delete, NONE) => BlocksKey::Delete,
        (Key::Character('d'), COMMAND) => BlocksKey::Duplicate,
        (Key::Up | Key::Down, COMMAND_SHIFT) => BlocksKey::Move { up: key == Key::Up },
        (Key::Up | Key::Down, NONE) => BlocksKey::Step { up: key == Key::Up },
        (Key::Up | Key::Down, SHIFT) => BlocksKey::Grow { up: key == Key::Up },
        (Key::Return, NONE) => BlocksKey::Edit,
        (Key::Escape, NONE) => BlocksKey::Clear,
        (Key::Character('c'), COMMAND) => BlocksKey::Copy,
        (Key::Character('x'), COMMAND) => BlocksKey::Cut,
        (Key::Character('v'), COMMAND) => BlocksKey::Paste,
        (Key::Character('a'), COMMAND) => BlocksKey::SelectAll,
        _ => return None,
    })
}

/// A shortcut the selected blocks take before the menu bar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlocksShortcut {
    /// ⌘Z, or redo with ⇧.
    Undo { redo: bool },
    /// Handled as a key (`blocks_key`), or by nothing when it does not apply.
    Key,
}

/// Whether the selected blocks take `key` ahead of the menu bar: ⌘ with A, D, Z, X, C, V, ↑
/// or ↓, whatever else is held.
pub fn blocks_shortcut(key: Key, held: Held) -> Option<BlocksShortcut> {
    if !held.command {
        return None;
    }
    match key {
        Key::Character('z') => Some(BlocksShortcut::Undo { redo: held.shift }),
        Key::Character('a' | 'd' | 'x' | 'c' | 'v') | Key::Up | Key::Down => {
            Some(BlocksShortcut::Key)
        }
        _ => None,
    }
}

/// What a key does to a focused media block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKey {
    /// ⌫ and ⌦ remove it.
    Remove,
    /// Return starts a line below it.
    LineBelow,
    /// ↑ and ↓ move to the block above or below.
    Step { up: bool },
}

/// What `key`, with no modifiers, does to a media block.
pub fn media_key(key: Key, held: Held) -> Option<MediaKey> {
    if held != NONE {
        return None;
    }
    Some(match key {
        Key::Backspace | Key::Delete => MediaKey::Remove,
        Key::Return => MediaKey::LineBelow,
        Key::Up | Key::Down => MediaKey::Step { up: key == Key::Up },
        _ => return None,
    })
}

/// The `/` menu entries ⌘⌥ and a digit turn a line into: 0 text, 1–3 a heading, 4 a to-do,
/// 5 a bulleted and 6 a numbered list, 8 code (indices into `page::edit::COMMANDS`).
fn digit_command(digit: char) -> Option<usize> {
    Some(match digit {
        '0' => 0,
        '1' => 1,
        '2' => 2,
        '3' => 3,
        '4' => 6,
        '5' => 4,
        '6' => 5,
        '8' => 8,
        _ => return None,
    })
}

/// What a shortcut does in the field being typed in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKey {
    /// ⌘B, ⌘I, ⌘E and ⇧⌘S: toggle the mark on the selection, removing it when all of it has it.
    Mark(Construct),
    /// ⌘K: link the selected text, or open the link's address.
    Link,
    /// ⌘⌥ and a digit: the line becomes `/` menu entry `n`.
    Turn(usize),
    /// ⌘Z and ⇧⌘Z, across blocks.
    Undo { redo: bool },
    /// ⇧⌘↑ and ⇧⌘↓ move the block.
    Move { up: bool },
    /// ⌘D duplicates the block below it.
    Duplicate,
    /// ⌘Return ticks or unticks a to-do.
    ToggleTodo,
}

/// What `key` does in a field when exactly these modifiers are `held`.
pub fn field_key(key: Key, held: Held) -> Option<FieldKey> {
    Some(match (key, held) {
        (Key::Character('b'), COMMAND) => FieldKey::Mark(Construct::Bold),
        (Key::Character('i'), COMMAND) => FieldKey::Mark(Construct::Italic),
        (Key::Character('e'), COMMAND) => FieldKey::Mark(Construct::Code),
        (Key::Character('s'), COMMAND_SHIFT) => FieldKey::Mark(Construct::Strike),
        (Key::Character('k'), COMMAND) => FieldKey::Link,
        (Key::Character(digit), COMMAND_OPTION) => FieldKey::Turn(digit_command(digit)?),
        (Key::Character('z'), COMMAND) => FieldKey::Undo { redo: false },
        (Key::Character('z'), COMMAND_SHIFT) => FieldKey::Undo { redo: true },
        (Key::Up | Key::Down, COMMAND_SHIFT) => FieldKey::Move {
            up: key.vertical()?,
        },
        (Key::Character('d'), COMMAND) => FieldKey::Duplicate,
        (Key::Return, COMMAND) => FieldKey::ToggleTodo,
        _ => return None,
    })
}

/// A command the field's text view issues after reading a key, such as a line break, tab,
/// caret move, deletion or Escape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Newline,
    Tab,
    Backtab,
    Cancel,
    Left,
    Right,
    Up,
    Down,
    DeleteBackward,
    DeleteForward,
    Other,
}

impl Command {
    /// A command as the FFI names it, by its key: Return for a line break, ⇧Tab, Escape, Backspace
    /// for a backward delete and Delete for a forward one.
    pub fn from_press(press: NrKeyPress) -> Self {
        match press.key {
            NrKey::Return => Self::Newline,
            NrKey::Tab if press.shift() => Self::Backtab,
            NrKey::Tab => Self::Tab,
            NrKey::Escape => Self::Cancel,
            NrKey::Left => Self::Left,
            NrKey::Right => Self::Right,
            NrKey::Up => Self::Up,
            NrKey::Down => Self::Down,
            NrKey::Backspace => Self::DeleteBackward,
            NrKey::Delete => Self::DeleteForward,
            _ => Self::Other,
        }
    }
}

/// Where a key moves focus from a table cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellMove {
    /// To the cell at `row`, `column`, after adding a row below the last when `add_row`.
    To {
        row: usize,
        column: usize,
        add_row: bool,
    },
    /// Out of the table: ⇧Tab in its first cell.
    Leave,
}

/// The cell `command` moves to from cell `row`, `column` in a `rows` by `columns` table.
/// Return goes down the column, adding a row past the last; ↑ and ↓ move between rows; Tab
/// moves along, adding a row past the last cell; ⇧Tab moves back. `None` leaves the command to
/// the cell.
pub fn cell_move(
    command: Command,
    row: usize,
    column: usize,
    rows: usize,
    columns: usize,
) -> Option<CellMove> {
    let last_row = row + 1 == rows;
    Some(match command {
        Command::Newline => CellMove::To {
            row: row + 1,
            column,
            add_row: last_row,
        },
        Command::Down if !last_row => CellMove::To {
            row: row + 1,
            column,
            add_row: false,
        },
        Command::Up if row > 0 => CellMove::To {
            row: row - 1,
            column,
            add_row: false,
        },
        Command::Tab if column + 1 < columns => CellMove::To {
            row,
            column: column + 1,
            add_row: false,
        },
        Command::Tab => CellMove::To {
            row: row + 1,
            column: 0,
            add_row: last_row,
        },
        Command::Backtab if column > 0 => CellMove::To {
            row,
            column: column - 1,
            add_row: false,
        },
        Command::Backtab if row > 0 => CellMove::To {
            row: row - 1,
            column: columns.saturating_sub(1),
            add_row: false,
        },
        Command::Backtab => CellMove::Leave,
        _ => return None,
    })
}

/// The `/` menu key for a command while the menu shows blocks: ↑ and ↓ cycle, Return and Tab
/// choose, Escape closes (`menus::keys::key_intent`). Every other command stays with the line.
pub fn slash_key(command: Command) -> Option<crate::menus::keys::Key> {
    use crate::menus::keys::Key as MenuKey;
    Some(match command {
        Command::Up => MenuKey::Up,
        Command::Down => MenuKey::Down,
        Command::Newline | Command::Tab => MenuKey::Return,
        Command::Cancel => MenuKey::Escape,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTION: Held = Held {
        option: true,
        ..NONE
    };
    const CONTROL: Held = Held {
        control: true,
        ..NONE
    };

    fn letter(typed: char) -> Key {
        Key::Character(typed)
    }

    fn press(key: NrKey) -> NrKeyPress {
        NrKeyPress::named(key)
    }

    fn by_place(letter: char) -> NrKeyPress {
        NrKeyPress {
            letter: letter as u32,
            ..press(NrKey::Other)
        }
    }

    #[test]
    fn the_boundary_names_keys_and_commands() {
        assert_eq!(Key::from_press(press(NrKey::Backspace)), Key::Backspace);
        assert_eq!(Key::from_press(press(NrKey::Delete)), Key::Delete);
        assert_eq!(Key::from_press(press(NrKey::Tab)), Key::Tab);
        assert_eq!(Key::from_press(by_place('D')), letter('d'));
        assert_eq!(Key::from_press(by_place('4')), letter('4'));
        // A key is named by its US-layout position, not by what it types.
        assert_eq!(
            Key::from_press(NrKeyPress {
                character: 'ф' as u32,
                ..by_place('a')
            }),
            letter('a')
        );
        assert_eq!(
            Key::from_press(NrKeyPress {
                character: '/' as u32,
                ..press(NrKey::Other)
            }),
            Key::Other
        );
        assert_eq!(Key::from_press(press(NrKey::Space)), Key::Other);
        assert_eq!(held(1 | 8), COMMAND_SHIFT);
        assert_eq!(held(1 | 4), COMMAND_OPTION);
        assert_eq!(Command::from_press(press(NrKey::Return)), Command::Newline);
        assert_eq!(Command::from_press(press(NrKey::Tab)), Command::Tab);
        assert_eq!(
            Command::from_press(NrKeyPress {
                held: crate::menus::SHIFT,
                ..press(NrKey::Tab)
            }),
            Command::Backtab
        );
        assert_eq!(
            Command::from_press(press(NrKey::Delete)),
            Command::DeleteForward
        );
        assert_eq!(Command::from_press(press(NrKey::Home)), Command::Other);
    }

    #[test]
    fn selected_blocks_answer_their_keys_with_exactly_those_held() {
        assert_eq!(blocks_key(Key::Backspace, NONE), Some(BlocksKey::Delete));
        assert_eq!(blocks_key(Key::Delete, NONE), Some(BlocksKey::Delete));
        assert_eq!(blocks_key(Key::Backspace, COMMAND), None);
        assert_eq!(blocks_key(letter('d'), COMMAND), Some(BlocksKey::Duplicate));
        assert_eq!(blocks_key(letter('d'), COMMAND_OPTION), None);
        assert_eq!(
            blocks_key(Key::Up, COMMAND_SHIFT),
            Some(BlocksKey::Move { up: true })
        );
        assert_eq!(
            blocks_key(Key::Down, COMMAND_SHIFT),
            Some(BlocksKey::Move { up: false })
        );
        assert_eq!(
            blocks_key(Key::Up, NONE),
            Some(BlocksKey::Step { up: true })
        );
        assert_eq!(
            blocks_key(Key::Down, SHIFT),
            Some(BlocksKey::Grow { up: false })
        );
        assert_eq!(blocks_key(Key::Up, COMMAND), None);
        assert_eq!(blocks_key(Key::Return, NONE), Some(BlocksKey::Edit));
        assert_eq!(blocks_key(Key::Return, SHIFT), None);
        assert_eq!(blocks_key(Key::Escape, NONE), Some(BlocksKey::Clear));
        assert_eq!(blocks_key(letter('c'), COMMAND), Some(BlocksKey::Copy));
        assert_eq!(blocks_key(letter('x'), COMMAND), Some(BlocksKey::Cut));
        assert_eq!(blocks_key(letter('v'), COMMAND), Some(BlocksKey::Paste));
        assert_eq!(blocks_key(letter('a'), COMMAND), Some(BlocksKey::SelectAll));
        assert_eq!(blocks_key(letter('a'), CONTROL), None);
        assert_eq!(
            blocks_key(letter('z'), COMMAND),
            None,
            "undo is a shortcut, not a key"
        );
        assert_eq!(blocks_key(Key::Left, NONE), None);
    }

    #[test]
    fn selected_blocks_take_their_shortcuts_before_the_menu_bar() {
        assert_eq!(
            blocks_shortcut(letter('z'), COMMAND),
            Some(BlocksShortcut::Undo { redo: false })
        );
        assert_eq!(
            blocks_shortcut(letter('z'), COMMAND_SHIFT),
            Some(BlocksShortcut::Undo { redo: true })
        );
        for key in [
            letter('a'),
            letter('d'),
            letter('x'),
            letter('c'),
            letter('v'),
            Key::Up,
            Key::Down,
        ] {
            assert_eq!(blocks_shortcut(key, COMMAND), Some(BlocksShortcut::Key));
            assert_eq!(
                blocks_shortcut(key, COMMAND_OPTION),
                Some(BlocksShortcut::Key),
                "whatever else is held"
            );
            assert_eq!(blocks_shortcut(key, NONE), None);
        }
        assert_eq!(blocks_shortcut(letter('b'), COMMAND), None);
        assert_eq!(blocks_shortcut(Key::Backspace, COMMAND), None);
    }

    #[test]
    fn a_media_block_takes_its_keys_with_nothing_held() {
        assert_eq!(media_key(Key::Backspace, NONE), Some(MediaKey::Remove));
        assert_eq!(media_key(Key::Delete, NONE), Some(MediaKey::Remove));
        assert_eq!(media_key(Key::Return, NONE), Some(MediaKey::LineBelow));
        assert_eq!(media_key(Key::Up, NONE), Some(MediaKey::Step { up: true }));
        assert_eq!(
            media_key(Key::Down, NONE),
            Some(MediaKey::Step { up: false })
        );
        assert_eq!(media_key(Key::Down, SHIFT), None);
        assert_eq!(media_key(Key::Left, NONE), None);
        assert_eq!(media_key(letter('a'), NONE), None);
    }

    #[test]
    fn a_field_takes_formatting_kinds_undo_and_block_shortcuts() {
        assert_eq!(
            field_key(letter('b'), COMMAND),
            Some(FieldKey::Mark(Construct::Bold))
        );
        assert_eq!(
            field_key(letter('i'), COMMAND),
            Some(FieldKey::Mark(Construct::Italic))
        );
        assert_eq!(
            field_key(letter('e'), COMMAND),
            Some(FieldKey::Mark(Construct::Code))
        );
        assert_eq!(
            field_key(letter('s'), COMMAND_SHIFT),
            Some(FieldKey::Mark(Construct::Strike))
        );
        assert_eq!(field_key(letter('s'), COMMAND), None);
        assert_eq!(field_key(letter('b'), COMMAND_SHIFT), None);
        assert_eq!(field_key(letter('k'), COMMAND), Some(FieldKey::Link));
        let turns: Vec<Option<FieldKey>> = "0123456789"
            .chars()
            .map(|digit| field_key(letter(digit), COMMAND_OPTION))
            .collect();
        assert_eq!(
            turns,
            [
                Some(0),
                Some(1),
                Some(2),
                Some(3),
                Some(6),
                Some(4),
                Some(5),
                None,
                Some(8),
                None
            ]
            .map(|command| command.map(FieldKey::Turn))
        );
        assert_eq!(field_key(letter('1'), COMMAND), None);
        assert_eq!(field_key(letter('1'), OPTION), None);
        assert_eq!(
            field_key(letter('z'), COMMAND),
            Some(FieldKey::Undo { redo: false })
        );
        assert_eq!(
            field_key(letter('z'), COMMAND_SHIFT),
            Some(FieldKey::Undo { redo: true })
        );
        assert_eq!(field_key(letter('z'), COMMAND_OPTION), None);
        assert_eq!(
            field_key(Key::Up, COMMAND_SHIFT),
            Some(FieldKey::Move { up: true })
        );
        assert_eq!(
            field_key(Key::Down, COMMAND_SHIFT),
            Some(FieldKey::Move { up: false })
        );
        assert_eq!(field_key(letter('d'), COMMAND), Some(FieldKey::Duplicate));
        assert_eq!(field_key(Key::Return, COMMAND), Some(FieldKey::ToggleTodo));
        assert_eq!(field_key(Key::Return, NONE), None);
    }

    #[test]
    fn a_cell_moves_down_its_column_and_along_its_row() {
        // A table of 3 rows by 2 columns.
        assert_eq!(
            cell_move(Command::Newline, 0, 1, 3, 2),
            Some(CellMove::To {
                row: 1,
                column: 1,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Newline, 2, 1, 3, 2),
            Some(CellMove::To {
                row: 3,
                column: 1,
                add_row: true
            })
        );
        assert_eq!(
            cell_move(Command::Down, 1, 0, 3, 2),
            Some(CellMove::To {
                row: 2,
                column: 0,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Down, 2, 0, 3, 2),
            None,
            "↓ in the last row is the cell's own"
        );
        assert_eq!(
            cell_move(Command::Up, 2, 1, 3, 2),
            Some(CellMove::To {
                row: 1,
                column: 1,
                add_row: false
            })
        );
        assert_eq!(cell_move(Command::Up, 0, 1, 3, 2), None);
        assert_eq!(
            cell_move(Command::Tab, 1, 0, 3, 2),
            Some(CellMove::To {
                row: 1,
                column: 1,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Tab, 1, 1, 3, 2),
            Some(CellMove::To {
                row: 2,
                column: 0,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Tab, 2, 1, 3, 2),
            Some(CellMove::To {
                row: 3,
                column: 0,
                add_row: true
            })
        );
        assert_eq!(
            cell_move(Command::Backtab, 1, 1, 3, 2),
            Some(CellMove::To {
                row: 1,
                column: 0,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Backtab, 1, 0, 3, 2),
            Some(CellMove::To {
                row: 0,
                column: 1,
                add_row: false
            })
        );
        assert_eq!(
            cell_move(Command::Backtab, 0, 0, 3, 2),
            Some(CellMove::Leave)
        );
        assert_eq!(cell_move(Command::Left, 1, 1, 3, 2), None);
        assert_eq!(cell_move(Command::DeleteBackward, 1, 1, 3, 2), None);
    }

    #[test]
    fn the_slash_menu_takes_the_arrows_return_tab_and_escape() {
        use crate::menus::keys::Key as MenuKey;
        assert_eq!(slash_key(Command::Up), Some(MenuKey::Up));
        assert_eq!(slash_key(Command::Down), Some(MenuKey::Down));
        assert_eq!(slash_key(Command::Newline), Some(MenuKey::Return));
        assert_eq!(slash_key(Command::Tab), Some(MenuKey::Return));
        assert_eq!(slash_key(Command::Cancel), Some(MenuKey::Escape));
        assert_eq!(slash_key(Command::Left), None);
        assert_eq!(slash_key(Command::Backtab), None);
        assert_eq!(slash_key(Command::DeleteBackward), None);
    }
}
