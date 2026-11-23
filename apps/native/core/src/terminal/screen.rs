//! The terminals on screen (ADR-0005): one filling a tab or beside its column, or a folder's
//! terminals tiled together. Tracks which file each shows, which one has the keyboard, and the
//! tile layout.

use crate::geometry::Rect;
use std::collections::HashMap;

/// Inset around a tile's grid so text never touches a rule or the window edge.
const TILE_INSET: f64 = 4.0;
/// Thickness of the rule between two tiles.
const RULE: f64 = 1.0;

/// The terminals on screen, keyed by the number each was shown as. A session has one screen:
/// a file shown again (the same terminal in the next tab) takes over its session, and the
/// previous screen releases nothing when it goes.
#[derive(Default)]
pub struct Shown {
    files: HashMap<u64, String>,
    /// The last number handed out; each newly shown terminal takes the next.
    last: u64,
    /// The terminal with the keyboard: the one shown last, or the one brought front since.
    /// 0 when it is gone or none is on screen.
    front: u64,
}

impl Shown {
    /// Puts the terminal file `id` on screen and returns its number.
    pub fn show(&mut self, id: &str) -> u64 {
        self.last += 1;
        self.files.insert(self.last, id.to_owned());
        self.front = self.last;
        self.last
    }

    /// The file shown by terminal `number`.
    pub fn file(&self, number: u64) -> Option<&str> {
        self.files.get(&number).map(String::as_str)
    }

    /// Gives terminal `number` the keyboard.
    pub fn bring_front(&mut self, number: u64) {
        if self.files.contains_key(&number) {
            self.front = number;
        }
    }

    /// The terminal with the keyboard; 0 for none.
    pub fn front(&self) -> u64 {
        self.front
    }

    /// The file of the terminal with the keyboard.
    pub fn front_file(&self) -> Option<&str> {
        self.file(self.front)
    }

    /// Takes terminal `number` off screen. Returns its file when no other screen shows it, so
    /// its session can release its drawing target.
    pub fn hide(&mut self, number: u64) -> Option<String> {
        let id = self.files.remove(&number)?;
        if self.front == number {
            self.front = 0;
        }
        (!self.files.values().any(|shown| *shown == id)).then_some(id)
    }

    /// Takes every terminal off screen.
    pub fn clear(&mut self) {
        self.files.clear();
        self.front = 0;
    }
}

/// Where a folder's terminals sit when they share the window.
#[derive(Debug, Default, PartialEq)]
pub struct Tiles {
    /// Each terminal's grid, in the folder's order.
    pub grids: Vec<Rect>,
    /// The rules between them.
    pub rules: Vec<Rect>,
}

/// Lays `count` terminals out in `area` in rows of equal height, with as many across as the
/// smallest square that holds them (two side by side, four as two rows of two). A shorter last
/// row spreads over the full width, since a terminal is more useful wide than tall. Grids are
/// inset from rules and window edges, and edges fall on whole points.
pub fn tiles(count: usize, area: Rect) -> Tiles {
    let mut laid = Tiles::default();
    if count == 0 {
        return laid;
    }
    let across = (1..=count).find(|side| side * side >= count).unwrap_or(1);
    let rows = count.div_ceil(across);
    for row in 0..rows {
        let top = edge(area.y, area.height, row, rows);
        let bottom = edge(area.y, area.height, row + 1, rows);
        if row > 0 {
            laid.rules.push(Rect::new(area.x, top, area.width, RULE));
        }
        let held = across.min(count - row * across);
        // A rule takes its thickness from the row under it.
        let rule_top = if row > 0 { RULE } else { 0.0 };
        lay_out_row(&mut laid, area, held, (top, bottom), rule_top);
    }
    laid
}

/// Where the `index`th of `parts` equal parts of `length` from `start` begins, on a whole point.
fn edge(start: f64, length: f64, index: usize, parts: usize) -> f64 {
    (start + length * index as f64 / parts as f64).round()
}

/// Lays `held` terminals side by side across `area` in the row from `top` to `bottom`, with a
/// rule between neighbours. The row's first `rule_top` points hold the rule above it.
fn lay_out_row(
    laid: &mut Tiles,
    area: Rect,
    held: usize,
    (top, bottom): (f64, f64),
    rule_top: f64,
) {
    for place in 0..held {
        let left = edge(area.x, area.width, place, held);
        let right = edge(area.x, area.width, place + 1, held);
        // A rule takes its thickness from the tile after it.
        let rule_left = if place > 0 { RULE } else { 0.0 };
        if place > 0 {
            laid.rules.push(Rect::new(left, top, RULE, bottom - top));
        }
        laid.grids.push(Rect::new(
            left + rule_left + TILE_INSET,
            top + rule_top + TILE_INSET,
            (right - left - rule_left - 2.0 * TILE_INSET).max(0.0),
            (bottom - top - rule_top - 2.0 * TILE_INSET).max(0.0),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_shown_again_keeps_its_session_on_screen() {
        let mut shown = Shown::default();
        assert_eq!((shown.front(), shown.front_file()), (0, None));
        let first = shown.show("a");
        let second = shown.show("b");
        assert_eq!((first, second), (1, 2));
        assert_eq!(shown.front_file(), Some("b"));
        shown.bring_front(first);
        assert_eq!((shown.front(), shown.file(first)), (first, Some("a")));
        // The next tab shows `a` too: the old screen goes and the session stays.
        let again = shown.show("a");
        assert_eq!(shown.hide(first), None);
        assert_eq!(shown.front(), again);
        // Hiding a file's last screen releases its session; an unknown number does nothing.
        assert_eq!(shown.hide(again), Some("a".to_owned()));
        assert_eq!(shown.front(), 0);
        assert_eq!(shown.hide(again), None);
        shown.bring_front(again);
        assert_eq!(shown.front(), 0);
        shown.clear();
        assert_eq!(shown.file(second), None);
    }

    #[test]
    fn terminals_tile_side_by_side_then_in_rows() {
        let area = Rect::new(0.0, 40.0, 1000.0, 600.0);
        assert_eq!(tiles(0, area), Tiles::default());
        // One fills the area, its grid inset from the window's edges.
        let one = tiles(1, area);
        assert_eq!(one.grids, [Rect::new(4.0, 44.0, 992.0, 592.0)]);
        assert!(one.rules.is_empty());
        // Two sit side by side with a rule between them.
        let two = tiles(2, area);
        assert_eq!(
            two.grids,
            [
                Rect::new(4.0, 44.0, 492.0, 592.0),
                Rect::new(505.0, 44.0, 491.0, 592.0)
            ]
        );
        assert_eq!(two.rules, [Rect::new(500.0, 40.0, 1.0, 600.0)]);
        // Three: two above, and the third as wide as the window below them.
        let three = tiles(3, area);
        assert_eq!(three.grids[2], Rect::new(4.0, 345.0, 992.0, 291.0));
        assert_eq!(
            three.rules,
            [
                Rect::new(500.0, 40.0, 1.0, 300.0),
                Rect::new(0.0, 340.0, 1000.0, 1.0)
            ]
        );
        // Four are two rows of two; seven are three across, the seventh alone in its row.
        let four = tiles(4, area);
        assert_eq!(four.grids[3], Rect::new(505.0, 345.0, 491.0, 291.0));
        assert_eq!(four.rules.len(), 3);
        let seven = tiles(7, area);
        assert_eq!(seven.grids.len(), 7);
        assert_eq!(seven.grids[6].width, 992.0);
        // No grid reaches into another or out of the area.
        for laid in [&one, &two, &three, &four, &seven] {
            for (index, a) in laid.grids.iter().enumerate() {
                assert!(a.x >= area.x && a.max_x() <= area.max_x());
                assert!(a.y >= area.y && a.max_y() <= area.max_y());
                for b in &laid.grids[index + 1..] {
                    let apart = a.max_x() <= b.x
                        || b.max_x() <= a.x
                        || a.max_y() <= b.y
                        || b.max_y() <= a.y;
                    assert!(apart, "{a:?} and {b:?}");
                }
            }
        }
    }
}
