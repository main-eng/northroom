//! Boundary for terminals opened together in one tab (`columns::Opened::together`): which file
//! each shows and where its grid sits (`terminal::screen::tiles`). The shell attaches each
//! session with `nr_terminal_attach_file`.

use crate::ffi::{with_columns, NrRect, NrText};
use crate::geometry::Rect;
use crate::Relock;
use std::sync::Mutex;

struct Placed {
    id: String,
    grid: Rect,
}

/// The tiles last laid out and the rules between them. The getters lend text from here.
static LAID: Mutex<(Vec<Placed>, Vec<Rect>)> = Mutex::new((Vec::new(), Vec::new()));

/// One terminal's tile. Text is valid until the next `nr_terminal_tiles_load`.
#[repr(C)]
pub struct NrTerminalTile {
    /// Its terminal file, which `nr_terminal_attach_file` shows.
    pub id: NrText,
    pub grid: NrRect,
}

fn nr_rect(rect: Rect) -> NrRect {
    NrRect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

/// The terminal files open together in the tab as (id, name); empty for a file open alone.
fn opened_together() -> Vec<(String, String)> {
    with_columns(|columns| columns.opened().map(|opened| opened.together.clone()))
        .flatten()
        .unwrap_or_default()
}

/// How many terminals are open together in the tab: two or more, or 0 for a file open alone.
#[no_mangle]
pub extern "C" fn nr_opened_terminals() -> usize {
    opened_together().len()
}

/// Lays out the terminals open together in `x`, `y`, `width` × `height` (window points) and
/// returns how many there are.
#[no_mangle]
pub extern "C" fn nr_terminal_tiles_load(x: f64, y: f64, width: f64, height: f64) -> usize {
    let together = opened_together();
    let laid = crate::terminal::screen::tiles(together.len(), Rect::new(x, y, width, height));
    let placed: Vec<Placed> = together
        .into_iter()
        .zip(&laid.grids)
        .map(|((id, _), grid)| Placed { id, grid: *grid })
        .collect();
    let count = placed.len();
    *LAID.relock() = (placed, laid.rules);
    count
}

/// Tile `index` of those last loaded; an empty one past the last.
#[no_mangle]
pub extern "C" fn nr_terminal_tile(index: usize) -> NrTerminalTile {
    let laid = LAID.relock();
    let Some(placed) = laid.0.get(index) else {
        return NrTerminalTile {
            id: NrText::borrowing(""),
            grid: nr_rect(Rect::default()),
        };
    };
    NrTerminalTile {
        id: NrText::borrowing(&placed.id),
        grid: nr_rect(placed.grid),
    }
}

/// How many rules sit between the tiles last loaded.
#[no_mangle]
pub extern "C" fn nr_terminal_tile_rules() -> usize {
    LAID.relock().1.len()
}

/// Rule `index` between the tiles last loaded; an empty one past the last.
#[no_mangle]
pub extern "C" fn nr_terminal_tile_rule(index: usize) -> NrRect {
    let laid = LAID.relock();
    nr_rect(laid.1.get(index).copied().unwrap_or_default())
}
