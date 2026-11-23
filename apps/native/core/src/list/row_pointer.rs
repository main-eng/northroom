//! What the mouse does to the rows of the list and of a column: which rows a press selects,
//! when a press becomes a drag, and which rows the drag carries.
//!
//! The rules name rows by their position on screen, top first (`Selected`). Between presses a
//! table keeps `Chosen`, the same selection by file id, so it survives a re-listing or a new
//! order and a removed row drops out of it.

/// How a press changes the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The row pressed alone.
    Replace,
    /// The rows from the anchor to the row pressed.
    Range,
    /// The row pressed in, or out when it was in.
    Toggle,
}

/// The mode the held keys ask for: ⇧ wins over ⌘ and Control.
pub fn mode(shift: bool, command: bool, control: bool) -> Mode {
    if shift {
        Mode::Range
    } else if command || control {
        Mode::Toggle
    } else {
        Mode::Replace
    }
}

/// The selected rows, top first, with the row a range grows from and the row last acted on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selected {
    pub rows: Vec<usize>,
    pub anchor: Option<usize>,
    pub focus: Option<usize>,
}

impl Selected {
    /// The selection as the next press finds it: the focus is kept while its row is selected
    /// and is otherwise the lowest selected row; the anchor is kept while its row is selected
    /// and is otherwise the focus.
    pub fn read(rows: &[usize], anchor: Option<usize>, focus: Option<usize>) -> Self {
        let mut rows = rows.to_vec();
        rows.sort_unstable();
        rows.dedup();
        let focus = focus
            .filter(|row| rows.contains(row))
            .or(rows.last().copied());
        let anchor = anchor.filter(|row| rows.contains(row)).or(focus);
        Self {
            rows,
            anchor,
            focus,
        }
    }

    /// The selection after a press on `target`, one of `count` rows on screen.
    pub fn press(&self, count: usize, target: usize, mode: Mode) -> Self {
        let anchor = self
            .anchor
            .filter(|&row| mode == Mode::Range && row < count);
        let mut rows = match (anchor, mode) {
            (Some(anchor), _) => (anchor.min(target)..=anchor.max(target)).collect(),
            (None, Mode::Toggle) if self.rows.contains(&target) => self
                .rows
                .iter()
                .copied()
                .filter(|&row| row != target)
                .collect(),
            (None, Mode::Toggle) => self.rows.iter().copied().chain([target]).collect(),
            (None, _) => vec![target],
        };
        // A row taken out leaves the focus on the last row of what was selected, as kept.
        let focus = if rows.contains(&target) {
            Some(target)
        } else {
            rows.last().copied()
        };
        rows.sort_unstable();
        Self {
            rows,
            anchor: Some(anchor.unwrap_or(target)),
            focus,
        }
    }

    /// What a drag that starts on `pressed` carries, and the selection it leaves: a selected
    /// row carries the whole selection; any other row is selected alone first and goes alone.
    pub fn pick_up(&self, pressed: usize) -> (Vec<usize>, Self) {
        if self.rows.contains(&pressed) {
            return (self.rows.clone(), self.clone());
        }
        let alone = Self {
            rows: vec![pressed],
            anchor: Some(pressed),
            focus: Some(pressed),
        };
        (vec![pressed], alone)
    }
}

/// How far the pointer must move on either axis, in screen pixels, before a press becomes a
/// drag. Matches Chromium at 2 px per point: 1 pt starts nothing and 1.5 pt starts a drag.
const DRAG_START_PIXELS: f64 = 3.0;

/// Whether a press that moved (`dx`, `dy`) points, on a screen of `scale` pixels per point, has
/// become a drag.
pub fn drag_starts(dx: f64, dy: f64, scale: f64) -> bool {
    dx.abs().max(dy.abs()) * scale >= DRAG_START_PIXELS
}

/// A table's selection kept by its rows' keys (a file's id): the rows selected, the one a range
/// grows from and the one last acted on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Chosen {
    pub ids: Vec<String>,
    pub anchor: Option<String>,
    pub focus: Option<String>,
}

impl Chosen {
    /// The selection among `keys`, the rows on screen top first, as the next press finds it
    /// (`Selected::read`).
    pub fn on(&self, keys: &[&str]) -> Selected {
        // Nothing chosen selects nothing, and leaves no anchor or focus to keep.
        if self.ids.is_empty() {
            return Selected::default();
        }
        let at = |id: &Option<String>| {
            id.as_deref()
                .and_then(|id| keys.iter().position(|key| *key == id))
        };
        let chosen: std::collections::HashSet<&str> = self.ids.iter().map(String::as_str).collect();
        let rows: Vec<usize> = (0..keys.len())
            .filter(|&row| chosen.contains(keys[row]))
            .collect();
        Selected::read(&rows, at(&self.anchor), at(&self.focus))
    }

    /// `selected`, a selection among `keys`, kept by the rows' keys.
    pub fn of(selected: &Selected, keys: &[&str]) -> Self {
        let key = |row: usize| keys.get(row).map(|key| (*key).to_owned());
        Self {
            ids: selected.rows.iter().filter_map(|&row| key(row)).collect(),
            anchor: selected.anchor.and_then(key),
            focus: selected.focus.and_then(key),
        }
    }

    /// The row at `row` alone, which the keys then act from.
    pub fn alone(keys: &[&str], row: usize) -> Self {
        Self::of(
            &Selected {
                rows: vec![row],
                anchor: Some(row),
                focus: Some(row),
            },
            keys,
        )
    }

    /// Every row, the anchor and the focus as they were.
    pub fn all(&self, keys: &[&str]) -> Self {
        Self {
            ids: keys.iter().map(|key| (*key).to_owned()).collect(),
            ..self.clone()
        }
    }

    /// The selection after a press on `target` (`Selected::press`).
    pub fn press(&self, keys: &[&str], target: usize, mode: Mode) -> Self {
        Self::of(&self.on(keys).press(keys.len(), target, mode), keys)
    }

    /// The selection after a key moves it: to the row `direction` lands on from the row last
    /// acted on (`row_keys::destination`), alone or with the rows from the anchor.
    pub fn step(
        &self,
        keys: &[&str],
        direction: crate::list::row_keys::Direction,
        range: bool,
    ) -> Self {
        let selected = self.on(keys);
        let mode = if range { Mode::Range } else { Mode::Replace };
        match crate::list::row_keys::destination(selected.focus, keys.len(), direction) {
            Some(target) => Self::of(&selected.press(keys.len(), target, mode), keys),
            None => self.clone(),
        }
    }

    /// Keeps only the rows `keeps` accepts, as after a re-listing.
    pub fn retain(&mut self, keeps: impl Fn(&str) -> bool) {
        self.ids.retain(|id| keeps(id));
        self.anchor = self.anchor.take().filter(|id| keeps(id));
        self.focus = self.focus.take().filter(|id| keeps(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(rows: &[usize], anchor: usize, focus: usize) -> Selected {
        Selected {
            rows: rows.to_vec(),
            anchor: Some(anchor),
            focus: Some(focus),
        }
    }

    #[test]
    fn shift_wins_over_command() {
        assert_eq!(mode(false, false, false), Mode::Replace);
        assert_eq!(mode(false, true, false), Mode::Toggle);
        assert_eq!(mode(false, false, true), Mode::Toggle);
        assert_eq!(mode(true, true, false), Mode::Range);
    }

    /// The presses of the screens check's `list-select` run, on its ten rows: gradient.png (7),
    /// ⌘ tone.wav (9), ⇧ Notes (5), ⌘ clip.mp4 (6), ⌘⇧ Assets (2).
    #[test]
    fn the_list_select_runs_presses_select_its_rows() {
        let mut selected = Selected::default().press(10, 7, Mode::Replace);
        assert_eq!(selected, at(&[7], 7, 7));
        selected = selected.press(10, 9, Mode::Toggle);
        assert_eq!(selected, at(&[7, 9], 9, 9));
        selected = selected.press(10, 5, Mode::Range);
        assert_eq!(selected, at(&[5, 6, 7, 8, 9], 9, 5));
        selected = selected.press(10, 6, Mode::Toggle);
        assert_eq!(selected.rows, vec![5, 7, 8, 9]);
        assert_eq!((selected.anchor, selected.focus), (Some(6), Some(9)));
        // The next press reads the selection first: the anchor's row is no longer selected,
        // so the range grows from the focus.
        let read = Selected::read(&selected.rows, selected.anchor, selected.focus);
        assert_eq!((read.anchor, read.focus), (Some(9), Some(9)));
        selected = read.press(10, 2, mode(true, true, false));
        assert_eq!(selected, at(&[2, 3, 4, 5, 6, 7, 8, 9], 9, 2));
    }

    #[test]
    fn a_range_without_an_anchor_is_the_row_alone() {
        let selected = Selected::default().press(10, 4, Mode::Range);
        assert_eq!(selected, at(&[4], 4, 4));
        // An anchor past the rows on screen is no anchor.
        let stale = at(&[12], 12, 12).press(10, 4, Mode::Range);
        assert_eq!(stale, at(&[4], 4, 4));
    }

    #[test]
    fn a_range_replaces_what_was_selected_and_keeps_its_anchor() {
        let selected = at(&[1, 8], 8, 8).press(10, 6, Mode::Range);
        assert_eq!(selected, at(&[6, 7, 8], 8, 6));
        assert_eq!(selected.press(10, 9, Mode::Range), at(&[8, 9], 8, 9));
    }

    #[test]
    fn taking_the_only_row_out_leaves_nothing_focused() {
        let selected = at(&[3], 3, 3).press(10, 3, Mode::Toggle);
        assert_eq!(
            selected,
            Selected {
                rows: vec![],
                anchor: Some(3),
                focus: None
            }
        );
        assert_eq!(
            Selected::read(&[], selected.anchor, selected.focus),
            Selected::default()
        );
    }

    #[test]
    fn a_selected_row_carries_the_selection_and_another_goes_alone() {
        let selected = at(&[7, 8, 9], 7, 9);
        assert_eq!(selected.pick_up(8), (vec![7, 8, 9], selected.clone()));
        assert_eq!(selected.pick_up(2), (vec![2], at(&[2], 2, 2)));
        assert_eq!(Selected::default().pick_up(0), (vec![0], at(&[0], 0, 0)));
    }

    #[test]
    fn a_drag_starts_at_three_pixels_on_either_axis() {
        assert!(!drag_starts(1.0, 0.0, 2.0));
        assert!(
            !drag_starts(1.0, 1.0, 2.0),
            "a diagonal is measured an axis at a time"
        );
        assert!(drag_starts(1.5, 0.0, 2.0));
        assert!(drag_starts(0.0, -1.5, 2.0));
        assert!(!drag_starts(2.0, 2.0, 1.0));
        assert!(drag_starts(-3.0, 0.0, 1.0));
    }

    const KEYS: [&str; 5] = ["a", "b", "c", "d", "e"];

    #[test]
    fn a_selection_is_kept_by_its_keys_through_a_new_order() {
        let chosen = Chosen::default()
            .press(&KEYS, 1, Mode::Replace)
            .press(&KEYS, 3, Mode::Toggle);
        assert_eq!(chosen.ids, ["b", "d"]);
        assert_eq!(
            (chosen.anchor.as_deref(), chosen.focus.as_deref()),
            (Some("d"), Some("d"))
        );
        // The same files in another order are still the ones selected.
        assert_eq!(chosen.on(&["d", "e", "b", "a"]), at(&[0, 2], 0, 0));
        assert_eq!(Chosen::default().all(&KEYS).on(&KEYS).rows, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn a_key_steps_from_the_row_last_acted_on() {
        use crate::list::row_keys::Direction;
        assert_eq!(
            Chosen::alone(&KEYS, 1)
                .step(&KEYS, Direction::Next, false)
                .on(&KEYS),
            at(&[2], 2, 2)
        );
        assert_eq!(
            Chosen::alone(&KEYS, 3)
                .step(&KEYS, Direction::Next, true)
                .on(&KEYS),
            at(&[3, 4], 3, 4)
        );
        // With nothing selected a move starts at the top; with no rows it changes nothing.
        assert_eq!(
            Chosen::default()
                .step(&KEYS, Direction::Previous, false)
                .on(&KEYS),
            at(&[0], 0, 0)
        );
        let chosen = Chosen::alone(&KEYS, 1);
        assert_eq!(chosen.step(&[], Direction::Next, false), chosen);
    }

    #[test]
    fn a_re_listing_forgets_the_rows_that_went() {
        let mut chosen = Chosen::alone(&KEYS, 1).press(&KEYS, 4, Mode::Range);
        assert_eq!(chosen.ids, ["b", "c", "d", "e"]);
        chosen.retain(|id| id == "e");
        assert_eq!(
            chosen,
            Chosen {
                ids: vec!["e".into()],
                anchor: None,
                focus: Some("e".into())
            }
        );
    }
}
