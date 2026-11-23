//! What a key does in the list and in a column.
//!
//! A shell names the key it was given (`Key`) and says which of ⌘, Control, ⌥ and ⇧ are held.
//!
//! Typing a name (`Intent::TypeAhead`) finds the first row whose name starts with the letters
//! typed lately (`Typed`, `typed_row`), as AppKit's type-select does.

use crate::columns::Table;
use std::time::{Duration, Instant};

/// The keys the rows answer to; any other key is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Right,
    Left,
    Down,
    Up,
    Return,
    F2,
    Space,
    /// The forward delete key.
    Delete,
    /// The key above Return.
    Backspace,
    Escape,
    Home,
    End,
    /// A key that types `char`.
    Character(char),
    Other,
}

/// Which rows a move takes: the one row, or the rows from the anchor to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Replace,
    Range,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Next,
    Previous,
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Move(Direction, Reach),
    /// Opens the folder in place, or goes to its first row when it is open.
    Expand,
    /// Closes the folder opened in place, or goes to the row of the folder this one is in.
    Collapse,
    Open,
    OpenParent,
    Rename,
    Delete,
    Clear,
    SelectAll,
    TypeAhead(char),
    /// In the columns: the column to the left takes the keys.
    PreviousColumn,
    /// In the columns: the open folder's column takes the keys.
    NextColumn,
}

/// The keys held with the key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    pub command: bool,
    pub control: bool,
    pub option: bool,
    pub shift: bool,
}

/// What `key` asks of the list.
pub fn list_intent(key: Key, held: Held) -> Option<Intent> {
    let command = held.command || held.control;
    let reach = if held.shift {
        Reach::Range
    } else {
        Reach::Replace
    };
    match key {
        // These mean the same whatever is held.
        Key::Right => Some(Intent::Expand),
        Key::Left => Some(Intent::Collapse),
        Key::Return | Key::F2 => Some(Intent::Rename),
        Key::Space | Key::Character(' ') => Some(Intent::Open),
        Key::Delete => Some(Intent::Delete),
        Key::Escape => Some(Intent::Clear),
        Key::Down if command => Some(Intent::Open),
        Key::Down => Some(Intent::Move(Direction::Next, reach)),
        Key::Up if command || held.option => Some(Intent::OpenParent),
        Key::Up => Some(Intent::Move(Direction::Previous, reach)),
        Key::Home => Some(Intent::Move(Direction::First, reach)),
        Key::End => Some(Intent::Move(Direction::Last, reach)),
        Key::Backspace => command.then_some(Intent::Delete),
        Key::Character(typed) if command && typed.eq_ignore_ascii_case(&'a') => {
            Some(Intent::SelectAll)
        }
        Key::Character(typed) if command && typed.eq_ignore_ascii_case(&'o') => Some(Intent::Open),
        Key::Character(typed) if !command && !held.option => Some(Intent::TypeAhead(typed)),
        Key::Character(_) | Key::Other => None,
    }
}

/// What `key` asks of a column: the keys that move between columns, and for every other key
/// what the list makes of it.
pub fn column_intent(key: Key, held: Held) -> Option<Intent> {
    let command = held.command || held.control;
    if !held.shift {
        match key {
            Key::Left if !command && !held.option => return Some(Intent::PreviousColumn),
            Key::Right if !command && !held.option => return Some(Intent::NextColumn),
            // Finder's "enclosing folder": the folder a column is in is the column to its left.
            Key::Up if command || held.option => return Some(Intent::PreviousColumn),
            _ => {}
        }
    }
    list_intent(key, held)
}

/// The row a move lands on among `count` rows on screen, from the row last acted on.
pub fn destination(focus: Option<usize>, count: usize, direction: Direction) -> Option<usize> {
    let last = count.checked_sub(1)?;
    Some(match direction {
        Direction::Next => focus.map_or(0, |row| row + 1).min(last),
        Direction::Previous => focus.map_or(0, |row| row.saturating_sub(1)),
        Direction::First => 0,
        Direction::Last => last,
    })
}

/// Letters typed within this interval form one search, as in AppKit's type-select.
pub const TYPED_TOGETHER: Duration = Duration::from_millis(1000);

/// The letters typed lately in one table, to find a row by its name.
#[derive(Clone, Debug, Default)]
pub struct Typed {
    letters: String,
    /// The table they were typed in and when the last was typed.
    last: Option<(Table, Instant)>,
}

impl Typed {
    /// Adds `letter` typed in `table` at `now`. It extends the search when typed in the same table
    /// within `TYPED_TOGETHER` of the previous letter, and starts a new one otherwise. Returns the
    /// search so far.
    pub fn add(&mut self, table: Table, letter: char, now: Instant) -> &str {
        let joins = self.last.is_some_and(|(typed_in, at)| {
            typed_in == table && now.saturating_duration_since(at) <= TYPED_TOGETHER
        });
        if !joins {
            self.letters.clear();
        }
        self.letters.push(letter);
        self.last = Some((table, now));
        &self.letters
    }
}

/// The first of `names` that starts with `typed`, ignoring case. A `None` name is a placeholder
/// row ("Empty folder") that typing never finds.
pub fn typed_row(names: &[Option<&str>], typed: &str) -> Option<usize> {
    let typed = typed.to_lowercase();
    names
        .iter()
        .position(|name| name.is_some_and(|name| name.to_lowercase().starts_with(&typed)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    const CONTROL: Held = Held {
        control: true,
        ..NONE
    };
    const OPTION: Held = Held {
        option: true,
        ..NONE
    };
    const SHIFT: Held = Held {
        shift: true,
        ..NONE
    };

    #[test]
    fn arrows_move_and_shift_reaches_from_the_anchor() {
        assert_eq!(
            list_intent(Key::Down, NONE),
            Some(Intent::Move(Direction::Next, Reach::Replace))
        );
        assert_eq!(
            list_intent(Key::Up, SHIFT),
            Some(Intent::Move(Direction::Previous, Reach::Range))
        );
        assert_eq!(
            list_intent(Key::Home, NONE),
            Some(Intent::Move(Direction::First, Reach::Replace))
        );
        assert_eq!(
            list_intent(Key::End, SHIFT),
            Some(Intent::Move(Direction::Last, Reach::Range))
        );
    }

    #[test]
    fn command_arrows_open_and_go_to_the_enclosing_folder() {
        assert_eq!(list_intent(Key::Down, COMMAND), Some(Intent::Open));
        assert_eq!(list_intent(Key::Down, CONTROL), Some(Intent::Open));
        assert_eq!(list_intent(Key::Up, COMMAND), Some(Intent::OpenParent));
        assert_eq!(list_intent(Key::Up, CONTROL), Some(Intent::OpenParent));
        assert_eq!(list_intent(Key::Up, OPTION), Some(Intent::OpenParent));
        assert_eq!(
            list_intent(Key::Character('o'), COMMAND),
            Some(Intent::Open)
        );
        assert_eq!(
            list_intent(Key::Character('O'), COMMAND),
            Some(Intent::Open)
        );
    }

    #[test]
    fn the_simple_keys_mean_the_same_whatever_is_held() {
        for held in [NONE, COMMAND, SHIFT, OPTION] {
            assert_eq!(list_intent(Key::Right, held), Some(Intent::Expand));
            assert_eq!(list_intent(Key::Left, held), Some(Intent::Collapse));
            assert_eq!(list_intent(Key::Return, held), Some(Intent::Rename));
            assert_eq!(list_intent(Key::F2, held), Some(Intent::Rename));
            assert_eq!(list_intent(Key::Space, held), Some(Intent::Open));
            assert_eq!(list_intent(Key::Delete, held), Some(Intent::Delete));
            assert_eq!(list_intent(Key::Escape, held), Some(Intent::Clear));
        }
    }

    #[test]
    fn backspace_deletes_only_with_command() {
        assert_eq!(list_intent(Key::Backspace, NONE), None);
        assert_eq!(list_intent(Key::Backspace, COMMAND), Some(Intent::Delete));
        assert_eq!(list_intent(Key::Backspace, CONTROL), Some(Intent::Delete));
    }

    #[test]
    fn letters_type_ahead_unless_command_or_option_is_held() {
        assert_eq!(
            list_intent(Key::Character('t'), NONE),
            Some(Intent::TypeAhead('t'))
        );
        assert_eq!(
            list_intent(Key::Character('T'), SHIFT),
            Some(Intent::TypeAhead('T'))
        );
        assert_eq!(
            list_intent(Key::Character('a'), COMMAND),
            Some(Intent::SelectAll)
        );
        assert_eq!(list_intent(Key::Character('t'), COMMAND), None);
        assert_eq!(list_intent(Key::Character('t'), OPTION), None);
        assert_eq!(list_intent(Key::Other, NONE), None);
    }

    #[test]
    fn typing_finds_the_first_name_that_starts_with_the_letters() {
        let names = [
            Some("All blocks"),
            Some("Assets"),
            None,
            Some("tagged.png"),
            Some("tone.wav"),
        ];
        assert_eq!(typed_row(&names, "t"), Some(3));
        assert_eq!(typed_row(&names, "TO"), Some(4));
        assert_eq!(typed_row(&names, "as"), Some(1));
        assert_eq!(typed_row(&names, "x"), None);
    }

    #[test]
    fn letters_typed_together_in_one_table_make_one_name() {
        let mut typed = Typed::default();
        let start = Instant::now();
        assert_eq!(typed.add(Table::List, 't', start), "t");
        assert_eq!(
            typed.add(Table::List, 'o', start + Duration::from_millis(400)),
            "to"
        );
        // Within a second of the last letter, not of the first.
        assert_eq!(
            typed.add(Table::List, 'n', start + Duration::from_millis(1400)),
            "ton"
        );
        assert_eq!(
            typed.add(Table::List, 'a', start + Duration::from_millis(2401)),
            "a"
        );
        // Another table starts a name of its own.
        assert_eq!(
            typed.add(Table::Column(0), 's', start + Duration::from_millis(2500)),
            "s"
        );
    }

    #[test]
    fn a_column_steps_sideways_and_keeps_the_lists_other_keys() {
        assert_eq!(column_intent(Key::Left, NONE), Some(Intent::PreviousColumn));
        assert_eq!(column_intent(Key::Right, NONE), Some(Intent::NextColumn));
        assert_eq!(
            column_intent(Key::Up, COMMAND),
            Some(Intent::PreviousColumn)
        );
        assert_eq!(column_intent(Key::Up, OPTION), Some(Intent::PreviousColumn));
        // With ⇧ held the column asks the list.
        assert_eq!(column_intent(Key::Left, SHIFT), Some(Intent::Collapse));
        assert_eq!(column_intent(Key::Left, COMMAND), Some(Intent::Collapse));
        assert_eq!(column_intent(Key::Down, COMMAND), Some(Intent::Open));
        assert_eq!(column_intent(Key::Return, NONE), Some(Intent::Rename));
        assert_eq!(
            column_intent(Key::Up, NONE),
            Some(Intent::Move(Direction::Previous, Reach::Replace))
        );
    }

    #[test]
    fn a_move_stops_at_the_ends_and_starts_at_the_top() {
        assert_eq!(destination(None, 5, Direction::Next), Some(0));
        assert_eq!(destination(None, 5, Direction::Previous), Some(0));
        assert_eq!(destination(Some(4), 5, Direction::Next), Some(4));
        assert_eq!(destination(Some(0), 5, Direction::Previous), Some(0));
        assert_eq!(destination(Some(2), 5, Direction::First), Some(0));
        assert_eq!(destination(Some(2), 5, Direction::Last), Some(4));
        assert_eq!(destination(Some(2), 0, Direction::Next), None);
    }
}
