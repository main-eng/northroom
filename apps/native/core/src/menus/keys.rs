//! How an open drawn menu responds to keys and the pointer, and where it opens, following
//! Radix's `DropdownMenu` and `ContextMenu`. A shell draws the menus, passes each key, hover
//! and open request here, and does what comes back.

/// A key while a menu is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Down,
    Up,
    Right,
    Left,
    Return,
    Other,
}

/// A row of the deepest open menu, as the keys see it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// It can be highlighted and chosen, so it is not a separator, a label or a disabled item.
    pub selectable: bool,
    pub has_submenu: bool,
}

/// What a key does. While a menu is open it takes every key; none reaches what is under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Nothing,
    /// Every open menu closes.
    CloseAll,
    /// The row is lit.
    Light(usize),
    /// The row's submenu opens beside it at once, with its first selectable row lit.
    OpenSubmenu(usize),
    /// The row is chosen.
    Choose(usize),
    /// The deepest submenu closes.
    CloseSubmenu,
}

/// ↓ and ↑ light the next and the previous selectable row, going round; with nothing lit they
/// start at the first or the last. → and Return open a lit row's submenu, and Return chooses a
/// lit row without one. ← closes a submenu (`depth` is how many menus are open under the
/// deepest). Escape closes them all.
pub fn key_intent(key: Key, rows: &[Row], lit: Option<usize>, depth: usize) -> Intent {
    let selectable: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.selectable)
        .map(|(index, _)| index)
        .collect();
    match key {
        Key::Escape => Intent::CloseAll,
        Key::Down | Key::Up => {
            if selectable.is_empty() {
                return Intent::Nothing;
            }
            let count = selectable.len();
            let current = lit.and_then(|lit| selectable.iter().position(|row| *row == lit));
            let next = match (current, key) {
                (Some(at), Key::Down) => (at + 1) % count,
                (Some(at), _) => (at + count - 1) % count,
                (None, Key::Down) => 0,
                (None, _) => count - 1,
            };
            Intent::Light(selectable[next])
        }
        Key::Right | Key::Return => match lit {
            Some(lit) if rows.get(lit).is_some_and(|row| row.has_submenu) => {
                Intent::OpenSubmenu(lit)
            }
            Some(lit) if key == Key::Return => Intent::Choose(lit),
            _ => Intent::Nothing,
        },
        Key::Left if depth > 0 => Intent::CloseSubmenu,
        Key::Left | Key::Other => Intent::Nothing,
    }
}

/// The row lit when a submenu opens from the keyboard.
pub fn first_selectable(rows: &[Row]) -> Option<usize> {
    rows.iter().position(|row| row.selectable)
}

/// What hovering a row of a menu, or none, does to the submenus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hover {
    /// The submenus deeper than this menu close.
    pub close_deeper: bool,
    /// The row's submenu opens after the pointer rests on it briefly.
    pub open: bool,
}

/// The pointer is over `row` of a menu, or over none of its rows. `deeper_open` says a submenu
/// is open under this menu, and `deeper_is_rows` that it belongs to this row. An open submenu
/// stays while its row is hovered or the pointer moves onto nothing, and closes when another
/// row is hovered. An enabled row with a submenu then opens it.
pub fn hover(row: Option<Row>, enabled: bool, deeper_open: bool, deeper_is_rows: bool) -> Hover {
    if deeper_open && (deeper_is_rows || row.is_none()) {
        return Hover {
            close_deeper: false,
            open: false,
        };
    }
    Hover {
        close_deeper: deeper_open,
        open: row.is_some_and(|row| row.has_submenu) && enabled,
    }
}

/// Whether a lit row stays lit when the pointer leaves its menu: one whose submenu is open,
/// while the pointer is in the submenu.
pub fn stays_lit_on_exit(has_submenu: bool) -> bool {
    has_submenu
}

// MARK: Where a menu opens

/// A rectangle with y running up from the bottom of the screen, as the Mac's screen has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn max_x(self) -> f64 {
        self.x + self.width
    }

    fn max_y(self) -> f64 {
        self.y + self.height
    }
}

/// Where a menu opens, and how it enters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Its lower-left corner.
    pub x: f64,
    pub y: f64,
    /// The direction it slides its last few points into place, in units of `motion::SLIDE`: x
    /// from the left (-1) or right (1), y down (1).
    pub slide_x: f64,
    pub slide_y: f64,
    /// The point it grows from, relative to its lower-left corner.
    pub origin_x: f64,
    pub origin_y: f64,
}

/// Places a `width` × `height` menu `gap` to the right of `anchor`, tops level, or to its left
/// when it would overflow `bounds` (the window content) on the right and fits on the left. It
/// is then moved, never resized, to stay inside `bounds`, and snapped to device pixels (`scale`
/// per point), as a browser places a popover.
///
/// A menu that hangs `below` its control grows from the middle of its top edge and slides
/// down; one beside the pointer or a row grows from the corner it opened at and slides in
/// from that side.
pub fn place(
    anchor: Rect,
    width: f64,
    height: f64,
    bounds: Rect,
    gap: f64,
    below: bool,
    scale: f64,
) -> Placement {
    let mut x = anchor.max_x() + gap;
    if x + width > bounds.max_x() && anchor.x - gap - width >= bounds.x {
        x = anchor.x - gap - width;
    }
    let (x, y) = held_inside(x, anchor, width, height, bounds, scale);
    if below {
        return Placement {
            x,
            y,
            slide_x: 0.0,
            slide_y: 1.0,
            origin_x: width / 2.0,
            origin_y: height,
        };
    }
    let opens_left = x < anchor.x - 1.0 && x + width <= anchor.x + 1.0;
    Placement {
        x,
        y,
        slide_x: if opens_left { 1.0 } else { -1.0 },
        slide_y: 0.0,
        origin_x: if opens_left { width } else { 0.0 },
        origin_y: (anchor.max_y() - y).clamp(0.0, height),
    }
}

/// A popover's lower-left corner for a left edge at `x` with its top level with `anchor`'s,
/// moved, never resized, to stay inside `bounds` and snapped to device pixels.
fn held_inside(
    x: f64,
    anchor: Rect,
    width: f64,
    height: f64,
    bounds: Rect,
    scale: f64,
) -> (f64, f64) {
    let x = x.min(bounds.max_x() - width).max(bounds.x);
    let y = (anchor.max_y() - height)
        .max(bounds.y)
        .min(bounds.max_y() - height);
    ((x * scale).round() / scale, (y * scale).round() / scale)
}

/// Places a `width` × `height` popover like `place`, but prefers the anchor's left: `gap` from
/// it, tops level, falling back to the right when the left would overflow `bounds` and the
/// right has room. Returns its lower-left corner. The colour picker opens this way beside its
/// swatch.
pub fn place_left(
    anchor: Rect,
    width: f64,
    height: f64,
    bounds: Rect,
    gap: f64,
    scale: f64,
) -> (f64, f64) {
    let mut x = anchor.x - gap - width;
    if x < bounds.x && anchor.max_x() + gap + width <= bounds.max_x() {
        x = anchor.max_x() + gap;
    }
    held_inside(x, anchor, width, height, bounds, scale)
}

/// Where a menu redrawn in another's place goes: its top-left corner (`left`, `top`) kept, and
/// no lower than `bottom`.
pub fn place_replacing(left: f64, top: f64, height: f64, bottom: f64) -> (f64, f64) {
    (left, bottom.max(top - height))
}

/// Highlight opacity for the hovered row: the text colour at 8%, or for a destructive row the
/// warning colour at 20% on dark themes and 10% on light ones.
pub fn light(destructive: bool, dark: bool) -> f64 {
    match (destructive, dark) {
        (false, _) => 0.08,
        (true, true) => 0.2,
        (true, false) => 0.1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: Row = Row {
        selectable: true,
        has_submenu: false,
    };
    const LINE: Row = Row {
        selectable: false,
        has_submenu: false,
    };
    const SUBMENU: Row = Row {
        selectable: true,
        has_submenu: true,
    };

    #[test]
    fn the_arrows_go_round_the_selectable_rows() {
        let rows = [ITEM, LINE, SUBMENU, ITEM];
        assert_eq!(key_intent(Key::Down, &rows, None, 0), Intent::Light(0));
        assert_eq!(key_intent(Key::Up, &rows, None, 0), Intent::Light(3));
        assert_eq!(
            key_intent(Key::Down, &rows, Some(0), 0),
            Intent::Light(2),
            "the line is passed over"
        );
        assert_eq!(key_intent(Key::Down, &rows, Some(3), 0), Intent::Light(0));
        assert_eq!(key_intent(Key::Up, &rows, Some(0), 0), Intent::Light(3));
        assert_eq!(key_intent(Key::Down, &[LINE], None, 0), Intent::Nothing);
        assert_eq!(key_intent(Key::Escape, &rows, Some(0), 2), Intent::CloseAll);
        assert_eq!(first_selectable(&rows[1..]), Some(1));
    }

    #[test]
    fn right_and_return_open_a_submenu_and_return_chooses() {
        let rows = [ITEM, SUBMENU];
        assert_eq!(
            key_intent(Key::Right, &rows, Some(1), 0),
            Intent::OpenSubmenu(1)
        );
        assert_eq!(
            key_intent(Key::Return, &rows, Some(1), 0),
            Intent::OpenSubmenu(1)
        );
        assert_eq!(
            key_intent(Key::Return, &rows, Some(0), 0),
            Intent::Choose(0)
        );
        assert_eq!(key_intent(Key::Right, &rows, Some(0), 0), Intent::Nothing);
        assert_eq!(key_intent(Key::Return, &rows, None, 0), Intent::Nothing);
        assert_eq!(
            key_intent(Key::Left, &rows, Some(0), 1),
            Intent::CloseSubmenu
        );
        assert_eq!(
            key_intent(Key::Left, &rows, Some(0), 0),
            Intent::Nothing,
            "the first menu stays"
        );
        assert_eq!(key_intent(Key::Other, &rows, Some(0), 0), Intent::Nothing);
    }

    #[test]
    fn a_submenu_stays_until_the_pointer_reaches_another_row() {
        let open = |row, deeper, own| hover(row, true, deeper, own);
        assert_eq!(
            open(Some(SUBMENU), false, false),
            Hover {
                close_deeper: false,
                open: true
            }
        );
        assert_eq!(
            open(Some(SUBMENU), true, true),
            Hover {
                close_deeper: false,
                open: false
            },
            "its own submenu stays"
        );
        assert_eq!(
            open(None, true, false),
            Hover {
                close_deeper: false,
                open: false
            },
            "off onto nothing keeps it"
        );
        assert_eq!(
            open(Some(ITEM), true, false),
            Hover {
                close_deeper: true,
                open: false
            }
        );
        assert_eq!(
            open(Some(SUBMENU), true, false),
            Hover {
                close_deeper: true,
                open: true
            }
        );
        assert_eq!(
            hover(Some(SUBMENU), false, false, false),
            Hover {
                close_deeper: false,
                open: false
            },
            "an unusable row opens nothing"
        );
        assert!(stays_lit_on_exit(true) && !stays_lit_on_exit(false));
    }

    const WINDOW: Rect = Rect {
        x: 100.0,
        y: 50.0,
        width: 1280.0,
        height: 800.0,
    };

    #[test]
    fn a_context_menu_opens_beside_the_pointer_and_flips_at_the_right_edge() {
        let at = |x: f64, y: f64| Rect {
            x,
            y,
            width: 0.0,
            height: 0.0,
        };
        let placed = place(at(500.0, 600.0), 192.0, 300.0, WINDOW, 2.0, false, 2.0);
        assert_eq!((placed.x, placed.y), (502.0, 300.0));
        assert_eq!(
            (
                placed.slide_x,
                placed.slide_y,
                placed.origin_x,
                placed.origin_y
            ),
            (-1.0, 0.0, 0.0, 300.0)
        );
        let flipped = place(at(1300.0, 600.0), 192.0, 300.0, WINDOW, 2.0, false, 2.0);
        assert_eq!(flipped.x, 1300.0 - 2.0 - 192.0);
        assert_eq!(
            (flipped.slide_x, flipped.origin_x),
            (1.0, 192.0),
            "it slides in from the right"
        );
        let low = place(at(500.0, 120.0), 192.0, 300.0, WINDOW, 2.0, false, 2.0);
        assert_eq!(low.y, 50.0, "moved up to stay in the window");
        assert_eq!(low.origin_y, 70.0, "it grows from the pointer's height");
        let narrow = Rect {
            x: 0.0,
            y: 0.0,
            width: 300.0,
            height: 800.0,
        };
        assert_eq!(
            place(at(200.0, 600.0), 250.0, 100.0, narrow, 2.0, false, 2.0).x,
            50.0,
            "kept inside when neither side fits"
        );
        assert_eq!(
            place(at(500.25, 600.3), 100.0, 100.0, WINDOW, 0.0, false, 2.0).x,
            500.5,
            "on whole device pixels"
        );
        assert_eq!(place_replacing(10.0, 700.0, 100.0, 50.0), (10.0, 600.0));
        // The colour picker: to the swatch's left, tops level; to its right only when the
        // left has no room.
        let swatch = Rect {
            x: 900.0,
            y: 500.0,
            width: 28.0,
            height: 28.0,
        };
        assert_eq!(
            place_left(swatch, 250.0, 300.0, WINDOW, 8.0, 2.0),
            (642.0, 228.0)
        );
        let near_left = Rect { x: 200.0, ..swatch };
        assert_eq!(
            place_left(near_left, 250.0, 300.0, WINDOW, 8.0, 2.0).0,
            236.0
        );
        let low = Rect { y: 60.0, ..swatch };
        assert_eq!(
            place_left(low, 250.0, 300.0, WINDOW, 8.0, 2.0).1,
            50.0,
            "kept in the window"
        );
        assert_eq!(place_replacing(10.0, 120.0, 100.0, 50.0), (10.0, 50.0));
    }

    #[test]
    fn a_menu_below_its_control_slides_down_from_its_top_middle() {
        let button = Rect {
            x: 400.0,
            y: 700.0,
            width: 0.0,
            height: 0.0,
        };
        let placed = place(button, 200.0, 100.0, WINDOW, 0.0, true, 2.0);
        assert_eq!(
            (
                placed.x,
                placed.y,
                placed.slide_x,
                placed.slide_y,
                placed.origin_x,
                placed.origin_y
            ),
            (400.0, 600.0, 0.0, 1.0, 100.0, 100.0)
        );
    }

    #[test]
    fn a_destructive_row_lights_in_the_warning_colour() {
        assert_eq!(
            (light(false, true), light(true, true), light(true, false)),
            (0.08, 0.2, 0.1)
        );
    }
}
