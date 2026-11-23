//! Pointer and key handling shared by the list and columns. A press selects on release, with
//! ⌘, ⇧ and Control as the core decides (`nr_rows_press`). A press that moves past the core's
//! threshold (`nr_drag_starts`) picks up the rows (`nr_rows_pick_up`). Keys, including
//! type-to-select, go to the core (`nr_rows_key`).

use super::rows_of;
use crate::keys::press as key_press;
use gpui::{Keystroke, Pixels, Point};
use northroom_core::ffi::nr_drag_starts;
use northroom_core::window::ffi::{NrRowIntent, nr_rows_key, nr_rows_pick_up, nr_rows_press};

#[derive(Clone, Copy, Debug)]
struct Press {
    row: i64,
    at: Point<Pixels>,
    held: u32,
    double: bool,
}

/// What a key asks of the view that has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Not handled here; let it propagate.
    PassOn,
    /// The selection changed; scroll `Some(row)` into view.
    Selection(Option<usize>),
    /// A folder expanded or collapsed in place; reload the list.
    Reload,
    Open(usize),
    OpenParent,
    Rename(usize),
    Trash,
    /// Column `n` takes the keys.
    Column(usize),
}

/// Pointer and key state for a table: `table` is -1 for the list, otherwise a column number.
#[derive(Debug)]
pub struct Rows {
    pub table: i64,
    press: Option<Press>,
}

impl Rows {
    pub fn new(table: i64) -> Self {
        Self { table, press: None }
    }

    /// Records a press on `row` (-1 for the space below the rows) at `at`, with `held`
    /// modifiers (`keys::held`), until it moves or is released.
    pub fn down(&mut self, row: i64, at: Point<Pixels>, held: u32, clicks: usize) {
        self.press = Some(Press {
            row,
            at,
            held,
            double: clicks == 2,
        });
    }

    pub fn is_pressed(&self) -> bool {
        self.press.is_some()
    }

    /// Handles a drag move to `at`. Once the press travels far enough on a row, returns the
    /// rows picked up (`nr_rows_pick_up`) to start a drag; empty when nothing may leave.
    pub fn moved(&mut self, at: Point<Pixels>, scale: f32) -> Option<Vec<usize>> {
        let press = self.press?;
        if press.row < 0 {
            return None;
        }
        let (dx, dy) = (f64::from(at.x - press.at.x), f64::from(at.y - press.at.y));
        if !nr_drag_starts(dx, dy, f64::from(scale)) {
            return None;
        }
        self.press = None;
        Some(rows_of(&nr_rows_pick_up(self.table, press.row as usize)))
    }

    /// Handles release: the press selects as the core says. Returns `Some(row)` on a double
    /// click, which opens the row; `None` when no press was followed.
    pub fn up(&mut self) -> Option<Option<usize>> {
        let press = self.press.take()?;
        nr_rows_press(self.table, press.row, press.held);
        Some((press.row >= 0 && press.double).then_some(press.row as usize))
    }

    /// Drops the followed press (a drag or menu took it).
    pub fn forget(&mut self) {
        self.press = None;
    }

    pub fn key(&mut self, keystroke: &Keystroke) -> Key {
        let asked = nr_rows_key(self.table, key_press(keystroke));
        let row = usize::try_from(asked.row).ok();
        match asked.intent {
            NrRowIntent::PassOn => Key::PassOn,
            NrRowIntent::Selection => Key::Selection(row),
            NrRowIntent::Reload => Key::Reload,
            NrRowIntent::Open => row.map_or(Key::PassOn, Key::Open),
            NrRowIntent::OpenParent => Key::OpenParent,
            NrRowIntent::Rename => row.map_or(Key::PassOn, Key::Rename),
            NrRowIntent::Trash => Key::Trash,
            NrRowIntent::Column => row.map_or(Key::PassOn, Key::Column),
        }
    }
}
