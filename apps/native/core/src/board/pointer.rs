//! What the pointer and the keys do on the board: the one card selected, the card with the keys and
//! whether its ring shows, the folder the pointer is over, the card a drag is over, how each
//! folder's art is moving between its poses, and when the zoom's label goes. A shell hands over
//! what it was given (which card was pressed, which key) and draws what comes back.

use crate::board::{folder_pose, FolderPose, FolderTarget, FOLDER_MOTION_SECONDS};
use crate::list::row_keys::Held;
use std::collections::{HashMap, HashSet};

/// A card of the board as the pointer and the keys see it.
#[derive(Clone, Copy, Debug)]
pub struct Card<'a> {
    pub id: &'a str,
    pub folder: bool,
    /// Files dragged within the drive can be dropped on it (`NrFileRow::takes_drops`).
    pub takes_drops: bool,
    /// A system item or standard location, which stays where it is.
    pub protected: bool,
}

/// The pointer over a card that plays: the card's id, and how far across the card it is, 0 at
/// its left edge and 1 at its right. The card shows the moment there.
#[derive(Clone, Debug, PartialEq)]
pub struct Skim {
    pub id: String,
    pub across: f64,
}

/// What a press on the board did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// The press was on no card: nothing is selected and no card has the keys.
    Cleared,
    /// The card is selected and has the keys, with no ring; the press may become a drag.
    Pressed,
    /// A double click: the card is to be opened.
    Open,
}

/// The keys the board answers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Tab,
    Space,
    Return,
    Delete,
    Backspace,
    Other,
}

/// What a key asks of the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAnswer {
    /// The board leaves the key to whoever is next.
    NotTaken,
    /// The board took it; draw it again.
    Taken,
    /// The keys go on past the board's last (or first) card: to the next view.
    Leave,
    /// Open the card at this index.
    Open(usize),
    /// Move the selected card to the trash, at once (⌘Z takes it back).
    Trash,
}

/// One step of `NR_BOARD_SHOW`, which puts the board in a state only a pointer reaches, for a
/// picture of it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShowStep {
    /// `zoom:4`: the board shows that many columns, with its zoom's label up.
    Zoom(usize),
    /// `hover:0`: the pointer is over that card.
    Hover(usize),
    /// `drop:0`: a drag is over that card.
    Drop(usize),
}

/// The steps `text` names, `+`-joined; one it cannot read is left out.
pub fn show_steps(text: &str) -> Vec<ShowStep> {
    text.split('+')
        .filter_map(|step| {
            let parts: Vec<&str> = step.split(':').filter(|part| !part.is_empty()).collect();
            let (name, rest) = parts.split_first()?;
            let first: f64 = rest.first()?.parse().ok()?;
            if first.is_nan() || first < 0.0 {
                return None;
            }
            let index = first as usize;
            match *name {
                "zoom" => Some(ShowStep::Zoom(index)),
                "hover" => Some(ShowStep::Hover(index)),
                "drop" => Some(ShowStep::Drop(index)),
                _ => None,
            }
        })
        .collect()
}

/// A folder's art on its way from one pose to another, set off at `start` (seconds on the
/// shell's clock).
#[derive(Clone, Copy, Debug)]
struct Motion {
    from: FolderPose,
    target: FolderTarget,
    start: f64,
}

impl Motion {
    fn pose(&self, now: f64) -> FolderPose {
        folder_pose(self.from, self.target, now - self.start)
    }

    fn arrived(&self, now: f64) -> bool {
        now - self.start > FOLDER_MOTION_SECONDS
    }
}

/// How the zoom's label shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hud {
    /// How long it takes to fade in, and later out.
    pub fade_seconds: f64,
    /// How far it comes down as it appears and goes back up as it leaves, in points.
    pub rise: f64,
    /// When it is to fade out (on the shell's clock); `None` keeps it up.
    pub hides_at: Option<f64>,
}

#[derive(Default)]
pub struct Pointer {
    selected: Option<String>,
    focused: Option<String>,
    /// A key moved the keys to the focused card: only then is its ring shown (`:focus-visible`).
    ring: bool,
    hovered: Option<String>,
    /// The card that plays (a film's or a video's) the pointer is skimming.
    skim: Option<Skim>,
    drop: Option<String>,
    /// The folders whose art is not at rest, by id.
    motions: HashMap<String, Motion>,
    hud_hides_at: Option<f64>,
}

fn index_of(cards: &[Card], id: Option<&str>) -> Option<usize> {
    id.and_then(|id| cards.iter().position(|card| card.id == id))
}

impl Pointer {
    pub fn selected(&self, cards: &[Card]) -> Option<usize> {
        index_of(cards, self.selected.as_deref())
    }

    pub fn focused(&self, cards: &[Card]) -> Option<usize> {
        index_of(cards, self.focused.as_deref())
    }

    /// The card that wears the keys' ring.
    pub fn ring(&self, cards: &[Card]) -> Option<usize> {
        self.ring.then(|| self.focused(cards)).flatten()
    }

    pub fn drop_card(&self, cards: &[Card]) -> Option<usize> {
        index_of(cards, self.drop.as_deref())
    }

    /// A press on `card` (`None`: the board's empty space), the `clicks`-th of a run. The
    /// board selects one card at a time, whatever keys are held.
    pub fn press(&mut self, card: Option<&str>, clicks: u32) -> Press {
        let Some(card) = card else {
            self.selected = None;
            self.focused = None;
            self.ring = false;
            return Press::Cleared;
        };
        // A press gives the card the keys, with no ring.
        self.focused = Some(card.to_owned());
        self.ring = false;
        if clicks == 2 {
            return Press::Open;
        }
        self.selected = Some(card.to_owned());
        Press::Pressed
    }

    /// `key` with `held` on a board of `cards`.
    pub fn key(&mut self, key: Key, held: Held, cards: &[Card]) -> KeyAnswer {
        let bare = held == Held::default();
        let focused = self.focused(cards);
        match key {
            // Tab and ⇧Tab move the keys card to card; past either end they leave the board.
            Key::Tab
                if bare
                    || held
                        == (Held {
                            shift: true,
                            ..Held::default()
                        }) =>
            {
                let step: i64 = if held.shift { -1 } else { 1 };
                self.move_focus(cards, focused, step)
            }
            // Space presses the focused card: it is selected.
            Key::Space if bare => {
                self.select_focused(cards, focused);
                KeyAnswer::Taken
            }
            // Return presses the focused card and opens it, or opens the one selected.
            Key::Return if bare => self
                .select_focused(cards, focused)
                .or_else(|| self.selected(cards))
                .map_or(KeyAnswer::Taken, KeyAnswer::Open),
            // Delete, and ⌘⌫, move the selected card to the trash, as they do a row of the list
            // or a column (`row_keys::list_intent`).
            Key::Delete | Key::Backspace
                if self.selected(cards).is_some() && (key == Key::Delete || held.command) =>
            {
                KeyAnswer::Trash
            }
            _ => KeyAnswer::NotTaken,
        }
    }

    /// Selects the focused card, when there is one, and answers its index.
    fn select_focused(&mut self, cards: &[Card], focused: Option<usize>) -> Option<usize> {
        let index = focused?;
        self.selected = Some(cards[index].id.to_owned());
        Some(index)
    }

    /// Moves the keys `step` cards on from `focused` (from the first card, or the last going
    /// back, when none has them), with the ring; past either end they leave the board.
    fn move_focus(&mut self, cards: &[Card], focused: Option<usize>, step: i64) -> KeyAnswer {
        let next = focused.map_or(if step > 0 { 0 } else { cards.len() as i64 - 1 }, |index| {
            index as i64 + step
        });
        match usize::try_from(next).ok().and_then(|next| cards.get(next)) {
            Some(card) => {
                self.focused = Some(card.id.to_owned());
                self.ring = true;
                KeyAnswer::Taken
            }
            None => {
                self.focused = None;
                self.ring = false;
                KeyAnswer::Leave
            }
        }
    }

    /// The pointer is over `card`, or off the cards; a folder's card lifts its sheets. Returns
    /// whether the folder under the pointer changed.
    pub fn hover(&mut self, card: Option<Card>) -> bool {
        self.set_hovered(
            card.filter(|card| card.folder)
                .map(|card| card.id.to_owned()),
        )
    }

    /// The pointer is skimming a card that plays, or none. Returns whether that changed and the
    /// card needs redrawing.
    pub fn skim(&mut self, skim: Option<Skim>) -> bool {
        let changed = self.skim != skim;
        self.skim = skim;
        changed
    }

    /// How far across `id`'s card the pointer is skimming it, when it is.
    pub fn skimmed(&self, id: &str) -> Option<f64> {
        self.skim
            .as_ref()
            .filter(|skim| skim.id == id)
            .map(|skim| skim.across)
    }

    /// The pointer is over `id`'s card, whatever it is (`NR_BOARD_SHOW`).
    pub fn set_hovered(&mut self, id: Option<String>) -> bool {
        let changed = self.hovered != id;
        self.hovered = id;
        changed
    }

    /// The files `dragged` are over `card`. A folder's card takes the drop, as does a running
    /// terminal's (which is handed the files), unless the card is one of the dragged files.
    /// Returns whether the drop target changed.
    pub fn drag_over(&mut self, card: Option<Card>, dragged: &[String]) -> bool {
        let over = card.filter(|card| {
            !dragged.is_empty() && card.takes_drops && !dragged.iter().any(|id| id == card.id)
        });
        self.set_drop(over.map(|card| card.id.to_owned()))
    }

    /// A drag is over `id`'s card, or over none.
    pub fn set_drop(&mut self, id: Option<String>) -> bool {
        let changed = self.drop != id;
        self.drop = id;
        changed
    }

    /// The cards a drag that has passed its threshold takes: those selected, but never the
    /// system's own folders, in board order.
    pub fn pick_up<'a>(&self, cards: &[Card<'a>]) -> Vec<&'a str> {
        cards
            .iter()
            .filter(|card| self.selected.as_deref() == Some(card.id) && !card.protected)
            .map(|card| card.id)
            .collect()
    }

    // Folder art in motion.

    fn target_of(&self, id: &str) -> FolderTarget {
        if self.drop.as_deref() == Some(id) {
            FolderTarget::Drop
        } else if self.hovered.as_deref() == Some(id) {
            FolderTarget::Hover
        } else {
            FolderTarget::Rest
        }
    }

    /// Sends each folder's art towards the pose it should now have, from wherever it is `now`.
    /// With motion `reduced` the art jumps there. Returns whether the shell must draw frames.
    pub fn retarget(&mut self, now: f64, reduced: bool) -> bool {
        let ids: HashSet<String> = self
            .motions
            .keys()
            .cloned()
            .chain(self.drop.clone())
            .chain(self.hovered.clone())
            .collect();
        for id in ids {
            let target = self.target_of(&id);
            let motion = self.motions.get(&id);
            if motion.map(|motion| motion.target) != Some(target) {
                let from = motion.map_or(FolderTarget::Rest.pose(), |motion| motion.pose(now));
                let start = if reduced {
                    now - FOLDER_MOTION_SECONDS
                } else {
                    now
                };
                self.motions.insert(
                    id,
                    Motion {
                        from,
                        target,
                        start,
                    },
                );
            }
        }
        !reduced
    }

    /// Whether `id`'s art is moving: the tests' view of what `step` forgets.
    #[cfg(test)]
    fn moving(&self, id: &str) -> bool {
        self.motions.contains_key(id)
    }

    /// Forgets the folders whose art has come back to rest by `now`, and says whether any art
    /// is still on its way.
    pub fn step(&mut self, now: f64) -> bool {
        self.motions
            .retain(|_, motion| !(motion.target == FolderTarget::Rest && motion.arrived(now)));
        !self.motions.values().all(|motion| motion.arrived(now))
    }

    /// How `id`'s art stands `now`.
    pub fn pose(&self, id: &str, now: f64) -> FolderPose {
        self.motions
            .get(id)
            .map_or(FolderTarget::Rest.pose(), |motion| motion.pose(now))
    }

    // The zoom's label.

    /// The zoom stepped `now`: its label shows, and goes `ZOOM_HUD_HOLD_MS` after the last step
    /// unless it is `staying`.
    pub fn show_hud(&mut self, now: f64, staying: bool) -> Hud {
        use crate::motion::{ZOOM_HUD_FADE_MS, ZOOM_HUD_HOLD_MS, ZOOM_HUD_RISE};
        self.hud_hides_at = (!staying).then_some(now + ZOOM_HUD_HOLD_MS / 1000.0);
        Hud {
            fade_seconds: ZOOM_HUD_FADE_MS / 1000.0,
            rise: ZOOM_HUD_RISE,
            hides_at: self.hud_hides_at,
        }
    }

    /// Called when a hide `deadline` arrives. Returns whether the label hides now; a later step
    /// may have put it off.
    pub fn hud_hides(&mut self, deadline: f64) -> bool {
        let hides = self.hud_hides_at == Some(deadline);
        if hides {
            self.hud_hides_at = None;
        }
        hides
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: &str) -> Card<'_> {
        Card {
            id,
            folder: false,
            takes_drops: false,
            protected: false,
        }
    }

    fn folder(id: &str) -> Card<'_> {
        Card {
            id,
            folder: true,
            takes_drops: true,
            protected: false,
        }
    }

    const SHIFT: Held = Held {
        command: false,
        control: false,
        option: false,
        shift: true,
    };

    #[test]
    fn a_press_selects_one_card_and_gives_it_the_keys_without_a_ring() {
        let cards = [card("a"), card("b")];
        let mut pointer = Pointer::default();
        assert_eq!(pointer.press(Some("b"), 1), Press::Pressed);
        assert_eq!(
            (
                pointer.selected(&cards),
                pointer.focused(&cards),
                pointer.ring(&cards)
            ),
            (Some(1), Some(1), None)
        );
        assert_eq!(pointer.press(Some("a"), 1), Press::Pressed);
        assert_eq!(pointer.selected(&cards), Some(0), "one card at a time");
        assert_eq!(pointer.press(None, 1), Press::Cleared);
        assert_eq!(
            (pointer.selected(&cards), pointer.focused(&cards)),
            (None, None)
        );
    }

    #[test]
    fn a_double_click_opens_and_leaves_the_selection() {
        let cards = [card("a"), card("b")];
        let mut pointer = Pointer::default();
        pointer.press(Some("a"), 1);
        assert_eq!(pointer.press(Some("b"), 2), Press::Open);
        assert_eq!(
            (pointer.selected(&cards), pointer.focused(&cards)),
            (Some(0), Some(1))
        );
        assert_eq!(
            pointer.press(Some("b"), 3),
            Press::Pressed,
            "a third click selects again"
        );
    }

    #[test]
    fn delete_and_command_backspace_trash_the_selected_card_only() {
        let cards = [card("a"), card("b")];
        let command = Held {
            command: true,
            ..Held::default()
        };
        let mut pointer = Pointer::default();
        assert_eq!(
            pointer.key(Key::Delete, Held::default(), &cards),
            KeyAnswer::NotTaken,
            "nothing selected"
        );
        pointer.press(Some("b"), 1);
        assert_eq!(
            pointer.key(Key::Delete, Held::default(), &cards),
            KeyAnswer::Trash
        );
        assert_eq!(
            pointer.key(Key::Backspace, command, &cards),
            KeyAnswer::Trash
        );
        assert_eq!(
            pointer.key(Key::Backspace, Held::default(), &cards),
            KeyAnswer::NotTaken,
            "a bare ⌫ trashes nothing"
        );
    }

    #[test]
    fn tab_moves_the_keys_card_to_card_with_a_ring_and_leaves_past_the_ends() {
        let cards = [card("a"), card("b")];
        let mut pointer = Pointer::default();
        assert_eq!(
            pointer.key(Key::Tab, Held::default(), &cards),
            KeyAnswer::Taken
        );
        assert_eq!(pointer.ring(&cards), Some(0));
        assert_eq!(
            pointer.key(Key::Tab, Held::default(), &cards),
            KeyAnswer::Taken
        );
        assert_eq!(pointer.ring(&cards), Some(1));
        assert_eq!(
            pointer.key(Key::Tab, Held::default(), &cards),
            KeyAnswer::Leave
        );
        assert_eq!(
            (pointer.focused(&cards), pointer.ring(&cards)),
            (None, None)
        );
        assert_eq!(
            pointer.key(Key::Tab, SHIFT, &cards),
            KeyAnswer::Taken,
            "⇧Tab comes in at the last card"
        );
        assert_eq!(pointer.ring(&cards), Some(1));
        pointer.key(Key::Tab, SHIFT, &cards);
        assert_eq!(pointer.key(Key::Tab, SHIFT, &cards), KeyAnswer::Leave);
        assert_eq!(
            pointer.key(
                Key::Tab,
                Held {
                    command: true,
                    ..Held::default()
                },
                &cards
            ),
            KeyAnswer::NotTaken
        );
        assert_eq!(
            Pointer::default().key(Key::Tab, SHIFT, &[]),
            KeyAnswer::Leave
        );
    }

    #[test]
    fn space_selects_the_focused_card_and_return_opens_it() {
        let cards = [card("a"), card("b")];
        let mut pointer = Pointer::default();
        assert_eq!(
            pointer.key(Key::Return, Held::default(), &cards),
            KeyAnswer::Taken,
            "nothing to open"
        );
        pointer.key(Key::Tab, Held::default(), &cards);
        pointer.key(Key::Tab, Held::default(), &cards);
        assert_eq!(
            pointer.key(Key::Space, Held::default(), &cards),
            KeyAnswer::Taken
        );
        assert_eq!(pointer.selected(&cards), Some(1));
        pointer.press(Some("a"), 1);
        pointer.key(Key::Tab, Held::default(), &cards);
        assert_eq!(
            pointer.key(Key::Return, Held::default(), &cards),
            KeyAnswer::Open(1)
        );
        assert_eq!(pointer.selected(&cards), Some(1));
        pointer.key(Key::Tab, Held::default(), &cards);
        assert_eq!(
            pointer.key(Key::Return, Held::default(), &cards),
            KeyAnswer::Open(1),
            "the one selected"
        );
        assert_eq!(pointer.key(Key::Space, SHIFT, &cards), KeyAnswer::NotTaken);
        assert_eq!(
            pointer.key(Key::Other, Held::default(), &cards),
            KeyAnswer::NotTaken
        );
    }

    #[test]
    fn one_card_that_plays_is_skimmed_at_a_time() {
        let mut pointer = Pointer::default();
        let at = |id: &str, across: f64| {
            Some(Skim {
                id: id.to_owned(),
                across,
            })
        };
        assert!(pointer.skim(at("film", 0.25)));
        assert!(!pointer.skim(at("film", 0.25)), "still there");
        assert_eq!(pointer.skimmed("film"), Some(0.25));
        assert!(pointer.skim(at("film", 0.5)), "moved across it");
        assert!(pointer.skim(at("clip", 0.5)));
        assert_eq!(pointer.skimmed("film"), None);
        assert!(pointer.skim(None));
        assert_eq!(pointer.skimmed("clip"), None);
    }

    #[test]
    fn only_a_folder_lifts_under_the_pointer() {
        let mut pointer = Pointer::default();
        assert!(!pointer.hover(Some(card("page"))));
        assert!(pointer.hover(Some(folder("f"))));
        assert!(!pointer.hover(Some(folder("f"))));
        assert!(pointer.hover(None));
    }

    #[test]
    fn a_drag_is_taken_by_a_card_that_takes_drops_and_is_not_dragged() {
        let cards = [folder("f"), card("page"), folder("g")];
        let mut pointer = Pointer::default();
        let dragged = vec!["page".to_owned()];
        assert!(pointer.drag_over(Some(cards[0]), &dragged));
        assert_eq!(pointer.drop_card(&cards), Some(0));
        assert!(pointer.drag_over(Some(cards[1]), &dragged));
        assert_eq!(pointer.drop_card(&cards), None);
        assert!(
            !pointer.drag_over(Some(cards[2]), &["g".to_owned()]),
            "a folder is never dropped into itself"
        );
        assert!(
            !pointer.drag_over(Some(cards[0]), &[]),
            "files from outside go into the folder shown"
        );
        pointer.drag_over(Some(cards[2]), &dragged);
        assert_eq!(pointer.drop_card(&cards), Some(2));
        assert!(pointer.set_drop(None));
        assert_eq!(pointer.drop_card(&cards), None);
    }

    #[test]
    fn a_drag_picks_up_the_selection_but_not_a_protected_card() {
        let cards = [
            card("a"),
            Card {
                protected: true,
                ..card("system")
            },
        ];
        let mut pointer = Pointer::default();
        pointer.press(Some("a"), 1);
        assert_eq!(pointer.pick_up(&cards), ["a"]);
        pointer.press(Some("system"), 1);
        assert!(pointer.pick_up(&cards).is_empty());
    }

    #[test]
    fn folder_art_moves_towards_its_target_and_forgets_it_at_rest() {
        let mut pointer = Pointer::default();
        pointer.set_hovered(Some("f".to_owned()));
        assert!(pointer.retarget(10.0, false));
        assert!(pointer.moving("f"));
        assert_eq!(pointer.pose("f", 10.0), FolderTarget::Rest.pose());
        assert_eq!(pointer.pose("f", 11.0), FolderTarget::Hover.pose());
        assert!(pointer.step(10.5), "still on its way");
        assert!(!pointer.step(11.0), "arrived, and kept while hovered");
        assert!(pointer.moving("f"));
        pointer.set_hovered(None);
        pointer.retarget(11.0, false);
        let back = pointer.pose("f", 11.1);
        assert!(back != FolderTarget::Hover.pose() && back != FolderTarget::Rest.pose());
        assert!(!pointer.step(12.0));
        assert!(!pointer.moving("f"), "back at rest it is forgotten");
        assert_eq!(pointer.pose("f", 12.0), FolderTarget::Rest.pose());
    }

    #[test]
    fn a_drop_outranks_the_hover_and_reduced_motion_arrives_at_once() {
        let mut pointer = Pointer::default();
        pointer.set_hovered(Some("f".to_owned()));
        pointer.set_drop(Some("f".to_owned()));
        assert!(!pointer.retarget(5.0, true), "no frames to draw");
        assert_eq!(pointer.pose("f", 5.0), FolderTarget::Drop.pose());
    }

    #[test]
    fn the_zoom_label_goes_after_the_last_step_unless_it_stays() {
        let mut pointer = Pointer::default();
        let first = pointer.show_hud(1.0, false);
        assert_eq!(
            first,
            Hud {
                fade_seconds: 0.15,
                rise: 4.0,
                hides_at: Some(1.9)
            }
        );
        let second = pointer.show_hud(1.5, false);
        assert!(!pointer.hud_hides(1.9), "a later step put it off");
        assert!(pointer.hud_hides(second.hides_at.expect("it goes")));
        assert!(!pointer.hud_hides(2.4), "once");
        assert_eq!(pointer.show_hud(3.0, true).hides_at, None);
    }

    #[test]
    fn board_show_names_states_by_card() {
        assert_eq!(
            show_steps("zoom:4+hover:0+drop:2"),
            [ShowStep::Zoom(4), ShowStep::Hover(0), ShowStep::Drop(2)]
        );
        assert_eq!(show_steps(""), []);
        assert_eq!(
            show_steps("drag:1:2+hover:x+zoom:-1+wiggle:1+hover::5"),
            [ShowStep::Hover(5)]
        );
    }
}
