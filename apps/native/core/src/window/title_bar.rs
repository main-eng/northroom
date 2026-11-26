//! The strip across the top of the window: Back and Forward, the open tabs, the new-tab button,
//! the sessions button and the update button.
//!
//! The shell measures text and passes the widths in. Everything else is decided here: layout,
//! hit testing, tooltips, tab scrolling, tab dragging, and when a tab shows its cross and where
//! its name fades.

use crate::board::Frame;
use crate::columns::TabId;

/// Where Back starts while the window buttons sit at the bar's left end. Without them, Back is
/// inset from the left by `EDGE`, as the controls are from the right.
const BACK_PAST_WINDOW_BUTTONS: f64 = 86.0;
/// Forward follows Back, and the tabs start 4 pt after Forward.
const TABS_PAST_BACK: f64 = BUTTON + BUTTON + 4.0;
/// The gap between tabs, and between the last tab and "+".
const TAB_GAP: f64 = 2.0;
/// Tab width limits.
const TAB_LEAST: f64 = 64.0;
const TAB_MOST: f64 = 176.0;
/// A bar button: 26 pt square, 8 pt below the bar's top.
const BUTTON: f64 = 26.0;
const BUTTON_TOP: f64 = 8.0;
/// A tab's chrome besides its name: 8 pt padding, a 14 pt glyph, 6 pt to the name and 8 pt
/// padding. The cross takes no room because it is drawn over the end of the name.
const TAB_PADDING: f64 = 8.0;
const TAB_GLYPH: f64 = 14.0;
const TAB_LABEL: f64 = TAB_PADDING + TAB_GLYPH + 6.0;
const TAB_CHROME: f64 = TAB_LABEL + TAB_PADDING;
/// A tab's cross: 16 pt square, 4 pt inside the tab's right edge.
const CLOSE: f64 = 16.0;
const CLOSE_INSET: f64 = 4.0;
/// A name is fully faded this far before a cross drawn over it.
const CLOSE_CLEAR: f64 = 2.0;
/// A name that does not fit fades over this length, capped at half its room.
const TITLE_FADE: f64 = 16.0;
/// Slack for measuring error: a name this far past its room still counts as fitting.
const TITLE_SLACK: f64 = 1.0;
/// The sessions button holds a 14 pt glyph. With text it becomes a pill: 8 pt padding at each
/// end and the text 6 pt after the glyph. It sits 4 pt left of the next button.
const SESSIONS_GLYPH: f64 = 14.0;
const PILL_PADDING: f64 = 8.0;
const PILL_GLYPH_GAP: f64 = 6.0;
const CONTROL_GAP: f64 = 4.0;
/// Inset of the controls from the bar's right end, and of "+" from the controls.
const EDGE: f64 = 8.0;

/// Text the bar shows.
pub const NEW_TAB: &str = "New tab";
pub const BACK: &str = "Back";
pub const FORWARD: &str = "Forward";

/// The bar's current content, as measured by the shell.
pub struct Shown<'a> {
    pub width: f64,
    /// Each tab's name width in the tab font.
    pub titles: &'a [f64],
    /// Whether each tab can close (`Columns::can_close_tab`); its cross shows on hover.
    pub closes: &'a [bool],
    /// The pointer position, while it is over the bar.
    pub pointer: Option<(f64, f64)>,
    pub active: usize,
    /// The active tab (`Columns::active_tab_id`); `None` with no tab open.
    pub active_id: Option<TabId>,
    /// The update button sits at the bar's right end.
    pub update: bool,
    pub sessions: SessionsShown,
    pub window_buttons: WindowButtons,
}

/// Whether the window buttons (close, minimise, zoom) sit at the bar's left end. Full screen
/// hides them, and Back, Forward and the tabs take their room.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButtons {
    Shown,
    Hidden,
}

impl WindowButtons {
    /// Where Back starts.
    fn back_x(self) -> f64 {
        match self {
            Self::Shown => BACK_PAST_WINDOW_BUTTONS,
            Self::Hidden => EDGE,
        }
    }
}

/// The sessions button as a shell measured it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SessionsShown {
    /// The width of the button's text (`sessions::ffi::SessionsButton`) in the pill font; 0 with
    /// no text.
    pub words: f64,
    /// How far the pill has grown, from 0 (glyph only) to 1 (full pill). The shell animates it.
    pub grown: f64,
}

/// What the bar's controls say in their tooltips, and whether Back and Forward can go.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tips<'a> {
    pub can_go_back: bool,
    pub can_go_forward: bool,
    /// The update button's label; empty while it is hidden.
    pub update: &'a str,
    /// The running-sessions summary (`sessions::summary`).
    pub sessions: &'a str,
}

/// One tab as it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Tab {
    pub frame: Frame,
    /// The glyph for its content.
    pub glyph: Frame,
    /// Where its name starts.
    pub label_x: f64,
    /// Where its name fades when it does not fit; `None` when it all shows.
    pub fade: Option<Fade>,
    /// Its cross, when it can close.
    pub close: Option<Close>,
}

/// Where a name fades: opaque up to `start`, gone at `end`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub start: f64,
    pub end: f64,
}

/// A tab's cross. It takes no room: it is drawn over the end of the name, which fades before
/// it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Close {
    /// Where a press closes the tab.
    pub frame: Frame,
    /// The pointer is on the tab, so the cross is drawn.
    pub shown: bool,
}

/// Positions of everything on the bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub back: Option<Frame>,
    pub forward: Option<Frame>,
    pub tabs: Vec<Tab>,
    /// The visible part of the tab row, and whether the tabs overflow it (clipped and
    /// scrolled).
    pub tabs_view: Option<Frame>,
    pub clipped: bool,
    pub plus: Option<Frame>,
    pub update: Option<Frame>,
    /// The sessions button, left of the update button, with its glyph and where its text starts
    /// as a pill.
    pub sessions: Option<Frame>,
    pub sessions_glyph: Option<Frame>,
    pub sessions_words_x: f64,
    /// How far the tabs are scrolled, and the row's length.
    pub offset: f64,
    pub length: f64,
}

/// What a point on the bar is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Back,
    Forward,
    Update,
    /// The sessions button, which opens the list of running sessions.
    Sessions,
    /// The cross of the tab at this index.
    Close(usize),
    Plus,
    Tab(usize),
    /// The bar itself, which moves the window.
    Bar,
}

/// A pressed tab. Once the pointer passes the drag threshold (`row_pointer::drag_starts`) the tab
/// follows it along the row and the other tabs make room at the nearest place, as browser tabs
/// do.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    tab: usize,
    /// Where the press was.
    pressed: (f64, f64),
    /// The press's offset from the tab's left edge.
    grab: f64,
    carried: bool,
    /// The pointer's x along the bar.
    pointer: f64,
}

/// The result of dragging a held tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Carry {
    /// No tab is dragged: none is held, or the pointer has not moved far enough.
    Still,
    /// The tab follows the pointer without changing place.
    Carried,
    /// The tab moved from the first index to the second.
    Moved(usize, usize),
}

/// Bar state kept between draws: tab scroll, the tab last scrolled into view, the last layout,
/// and the held tab.
#[derive(Debug, Default)]
pub struct TitleBar {
    revealed: Option<TabId>,
    layout: Layout,
    held: Option<Held>,
    /// The last width of the sessions button's text.
    pill_words: f64,
}

fn button(x: f64) -> Frame {
    Frame {
        x,
        y: BUTTON_TOP,
        width: BUTTON,
        height: BUTTON,
    }
}

/// The sessions button ending at `right`, grown `grown` of the way from glyph to a pill holding
/// text `words` wide. Returns its frame, its glyph, and where its text starts.
fn sessions_button(words: f64, grown: f64, right: f64) -> (Frame, Frame, f64) {
    let pill = PILL_PADDING + SESSIONS_GLYPH + PILL_GLYPH_GAP + words + PILL_PADDING;
    let width = BUTTON + (pill - BUTTON) * grown;
    let frame = Frame {
        x: right - width,
        width,
        ..button(0.0)
    };
    // The glyph is centred alone and sits at the padding in the pill.
    let alone = (BUTTON - SESSIONS_GLYPH) / 2.0;
    let glyph = Frame {
        x: frame.x + alone + (PILL_PADDING - alone) * grown,
        y: BUTTON_TOP + alone,
        width: SESSIONS_GLYPH,
        height: SESSIONS_GLYPH,
    };
    (frame, glyph, glyph.x + SESSIONS_GLYPH + PILL_GLYPH_GAP)
}

fn contains(frame: Option<Frame>, x: f64, y: f64) -> bool {
    frame.is_some_and(|frame| {
        x >= frame.x && x < frame.x + frame.width && y >= frame.y && y < frame.y + frame.height
    })
}

impl TitleBar {
    /// Lays the bar out for `shown`. A newly active tab (opened, chosen, or the neighbour of a
    /// closed active tab) is scrolled into view. Otherwise the scroll stays put, clamped to the
    /// row, so closing another tab leaves the rest in place.
    pub fn lay_out(&mut self, shown: &Shown) -> &Layout {
        let right = shown.width - EDGE;
        let update = shown.update.then(|| button(right - BUTTON));
        let sessions_right = update.map_or(right, |update| update.x - CONTROL_GAP);
        // A pill that lost its text shrinks at the width of its last text.
        if shown.sessions.words > 0.0 {
            self.pill_words = shown.sessions.words;
        }
        let (sessions, sessions_glyph, sessions_words_x) = sessions_button(
            self.pill_words,
            shown.sessions.grown.clamp(0.0, 1.0),
            sessions_right,
        );
        let back = shown.window_buttons.back_x();
        let origin = back + TABS_PAST_BACK;
        let mut layout = Layout {
            back: Some(button(back)),
            forward: Some(button(back + BUTTON)),
            update,
            sessions: Some(sessions),
            sessions_glyph: Some(sessions_glyph),
            sessions_words_x,
            offset: self.layout.offset,
            ..Layout::default()
        };
        let wanted: Vec<f64> = shown
            .titles
            .iter()
            .map(|title| title + TAB_CHROME)
            .collect();
        // The row ends where "+" and its gap meet the sessions button.
        let end = sessions.x - EDGE;
        let available = (end - BUTTON - TAB_GAP - origin).max(TAB_LEAST);
        let strip = strip(&wanted, available, TAB_GAP, TAB_LEAST, TAB_MOST);
        let view = strip.length.min(available);
        layout.tabs_view = Some(Frame {
            x: origin,
            y: 0.0,
            width: view,
            height: crate::window::TITLE_BAR_HEIGHT,
        });
        layout.clipped = strip.length > available;
        layout.length = strip.length;
        if shown.active_id != self.revealed {
            self.revealed = shown.active_id;
            let before: f64 =
                strip.widths.iter().take(shown.active).sum::<f64>() + TAB_GAP * shown.active as f64;
            let width = strip.widths.get(shown.active).copied().unwrap_or(0.0);
            layout.offset = crate::scroll::route::reveal(
                layout.offset,
                view,
                strip.length,
                before,
                before + width,
            );
        }
        layout.offset = crate::scroll::route::clamp(layout.offset, view, strip.length);
        self.held = self.held.filter(|held| held.tab < shown.titles.len());
        let carried = self.held.filter(|held| held.carried);
        // The pointer is on a tab only inside the visible part of the row.
        let pointer = shown
            .pointer
            .filter(|&(x, y)| contains(layout.tabs_view, x, y));
        let mut x = origin - layout.offset;
        for (index, &width) in strip.widths.iter().enumerate() {
            let left = match carried {
                Some(held) if held.tab == index => carried_left(held, width, origin, view, &layout),
                _ => x,
            };
            let frame = Frame {
                x: left,
                y: BUTTON_TOP,
                width,
                height: BUTTON,
            };
            let pointed = pointer.is_some_and(|(x, y)| contains(Some(frame), x, y));
            let close = shown.closes[index].then(|| Close {
                frame: close_frame(frame),
                shown: pointed,
            });
            layout
                .tabs
                .push(place_tab(frame, shown.titles[index], close));
            x += width + TAB_GAP;
        }
        layout.plus = Some(button(origin + view + TAB_GAP));
        self.layout = layout;
        &self.layout
    }

    /// The bar as last laid out.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Holds tab `tab`, pressed at (`x`, `y`). It is dragged once the pointer moves.
    pub fn hold(&mut self, tab: usize, x: f64, y: f64) {
        self.held = self.layout.tabs.get(tab).map(|placed| Held {
            tab,
            pressed: (x, y),
            grab: x - placed.frame.x,
            carried: false,
            pointer: x,
        });
    }

    /// Moves the pressed pointer to (`x`, `y`) on a screen of `scale` pixels per point. Past the
    /// drag threshold the held tab follows it and takes the nearest place. The row is reordered
    /// now, so a second carry before the next layout counts from the new order.
    pub fn carry(&mut self, x: f64, y: f64, scale: f64) -> Carry {
        let Some(mut held) = self.held else {
            return Carry::Still;
        };
        let (dx, dy) = (x - held.pressed.0, y - held.pressed.1);
        if !held.carried && !crate::list::row_pointer::drag_starts(dx, dy, scale) {
            return Carry::Still;
        }
        held.carried = true;
        held.pointer = x;
        let from = held.tab;
        let to = self.place_for(held);
        held.tab = to;
        self.held = Some(held);
        if to == from {
            return Carry::Carried;
        }
        let tab = self.layout.tabs.remove(from);
        self.layout.tabs.insert(to, tab);
        Carry::Moved(from, to)
    }

    /// Releases the held tab where it is. Returns whether it was dragged.
    pub fn let_go(&mut self) -> bool {
        self.held.take().is_some_and(|held| held.carried)
    }

    /// The tab being dragged, if any. It is drawn over the others.
    pub fn carried(&self) -> Option<usize> {
        self.held.filter(|held| held.carried).map(|held| held.tab)
    }

    /// The tab index nearest where `held` is drawn.
    fn place_for(&self, held: Held) -> usize {
        let tabs = &self.layout.tabs;
        let Some(view) = self.layout.tabs_view else {
            return held.tab;
        };
        let width = tabs[held.tab].frame.width;
        let left = carried_left(held, width, view.x, view.width, &self.layout);
        let others: Vec<f64> = tabs
            .iter()
            .enumerate()
            .filter(|&(index, _)| index != held.tab)
            .map(|(_, tab)| tab.frame.width)
            .collect();
        nearest_place(&others, view.x - self.layout.offset, TAB_GAP, left)
    }

    /// Scrolls the tabs by sideways wheel travel `across` (positive is right) in `unit`
    /// (`scroll::route::wheel_scrolled`), clamped to the row. Returns whether they moved.
    pub fn scroll(&mut self, across: f64, unit: u32) -> bool {
        let Some(view) = self.layout.tabs_view else {
            return false;
        };
        let moved = crate::scroll::route::wheel_scrolled(
            self.layout.offset,
            across,
            unit,
            view.width,
            self.layout.length,
        );
        let changed = moved != self.layout.offset;
        self.layout.offset = moved;
        changed
    }

    /// Whether (`x`, `y`) is over the tab row, where sideways scrolling applies.
    pub fn over_tabs(&self, x: f64, y: f64) -> bool {
        contains(self.layout.tabs_view, x, y)
    }

    /// The control at (`x`, `y`). Tabs and crosses count only inside the visible part of the
    /// row.
    pub fn hit(&self, x: f64, y: f64) -> Control {
        let layout = &self.layout;
        let in_tabs = contains(layout.tabs_view, x, y);
        let tab = || {
            layout
                .tabs
                .iter()
                .position(|tab| contains(Some(tab.frame), x, y))
                .filter(|_| in_tabs)
        };
        if contains(layout.back, x, y) {
            Control::Back
        } else if contains(layout.forward, x, y) {
            Control::Forward
        } else if contains(layout.update, x, y) {
            Control::Update
        } else if contains(layout.sessions, x, y) {
            Control::Sessions
        } else if let Some(index) = tab().filter(|&index| layout.tabs[index].closes_at(x, y)) {
            Control::Close(index)
        } else if contains(layout.plus, x, y) {
            Control::Plus
        } else if let Some(index) = tab() {
            Control::Tab(index)
        } else {
            Control::Bar
        }
    }

    /// The tab under (`x`, `y`), cross included. A right click opens its menu
    /// (`menus::tab_menu`); a middle-button release closes it unless it is the last.
    pub fn tab_at(&self, x: f64, y: f64) -> Option<usize> {
        match self.hit(x, y) {
            Control::Close(tab) | Control::Tab(tab) => Some(tab),
            _ => None,
        }
    }

    /// The tooltip and frame for the control under (`x`, `y`), from what each control says
    /// (`Tips`). Disabled Back or Forward and tabs have none.
    pub fn tooltip(&self, x: f64, y: f64, tips: &Tips) -> Option<(Frame, String)> {
        let layout = &self.layout;
        let (frame, words) = match self.hit(x, y) {
            Control::Plus => (layout.plus, NEW_TAB),
            Control::Update => (layout.update, tips.update),
            Control::Sessions => (layout.sessions, tips.sessions),
            Control::Back if tips.can_go_back => (layout.back, BACK),
            Control::Forward if tips.can_go_forward => (layout.forward, FORWARD),
            _ => return None,
        };
        (!words.is_empty()).then(|| {
            (
                frame.expect("the control under the point has a frame"),
                words.to_owned(),
            )
        })
    }
}

/// The left edge of the dragged tab `held`, `width` wide: kept under the pointer at the press
/// offset and inside the visible row, `view` wide from `origin`.
fn carried_left(held: Held, width: f64, origin: f64, view: f64, layout: &Layout) -> f64 {
    let first = origin.max(origin - layout.offset);
    let last = (origin + view).min(origin - layout.offset + layout.length) - width;
    (held.pointer - held.grab).clamp(first, last.max(first))
}

/// The place for a tab with its left edge at `left`, among tabs `others` wide laid from `start`
/// with `gap` between them: the place whose left edge is nearest, the first on a tie.
pub fn nearest_place(others: &[f64], start: f64, gap: f64, left: f64) -> usize {
    let mut edge = start;
    let mut nearest = (0, (left - start).abs());
    for (index, width) in others.iter().enumerate() {
        edge += width + gap;
        let distance = (left - edge).abs();
        if distance < nearest.1 {
            nearest = (index + 1, distance);
        }
    }
    nearest.0
}

impl Tab {
    /// Whether a press at (`x`, `y`) is on its cross.
    fn closes_at(&self, x: f64, y: f64) -> bool {
        contains(self.close.map(|close| close.frame), x, y)
    }
}

/// The cross frame for `tab`: inside its right edge, vertically centred.
fn close_frame(tab: Frame) -> Frame {
    Frame {
        x: tab.x + tab.width - CLOSE_INSET - CLOSE,
        y: tab.y + (tab.height - CLOSE) / 2.0,
        width: CLOSE,
        height: CLOSE,
    }
}

/// The tab at `frame` with a name `title` wide. The name runs from the glyph to the padding, or
/// to just before a drawn cross, and fades where it does not fit.
fn place_tab(frame: Frame, title: f64, close: Option<Close>) -> Tab {
    let label_x = frame.x + TAB_LABEL;
    let limit = match close {
        Some(Close {
            frame: cross,
            shown: true,
        }) => cross.x - CLOSE_CLEAR,
        Some(Close { shown: false, .. }) | None => frame.x + frame.width - TAB_PADDING,
    };
    Tab {
        frame,
        glyph: Frame {
            x: frame.x + TAB_PADDING,
            y: frame.y + (frame.height - TAB_GLYPH) / 2.0,
            width: TAB_GLYPH,
            height: TAB_GLYPH,
        },
        label_x,
        fade: fade(label_x, title, limit),
        close,
    }
}

/// The fade for a name `title` wide from `start` so it is gone at `limit`; `None` when it ends
/// by then.
fn fade(start: f64, title: f64, limit: f64) -> Option<Fade> {
    if start + title <= limit + TITLE_SLACK {
        return None;
    }
    let room = (limit - start).max(0.0);
    Some(Fade {
        start: limit - TITLE_FADE.min(room / 2.0),
        end: limit,
    })
}

/// Title bar tabs laid out in `available` points with `gap` between them.
#[derive(Clone, Debug, PartialEq)]
pub struct Strip {
    /// Each tab's width.
    pub widths: Vec<f64>,
    /// Total length with gaps; more than `available` when the row must scroll.
    pub length: f64,
}

/// Lays out tabs that want `wanted` points each, clamped to `least`..=`most`. When they do not
/// fit, they shrink like flex items (`flex-shrink: 1`): each gives up room in proportion to its
/// width, and a tab that reaches `least` stops while the others give up the rest. If every tab
/// is at `least` and they still overflow, the row is longer than `available` and scrolls.
pub fn strip(wanted: &[f64], available: f64, gap: f64, least: f64, most: f64) -> Strip {
    let base: Vec<f64> = wanted
        .iter()
        .map(|width| width.clamp(least, most))
        .collect();
    let gaps = gap * base.len().saturating_sub(1) as f64;
    let mut tabs = Shrinking {
        widths: base.clone(),
        frozen: vec![false; base.len()],
        base,
        least,
    };
    loop {
        let room = available - gaps - tabs.frozen_width();
        let asked = tabs.free_width();
        if asked <= room || asked == 0.0 {
            break;
        }
        // Every unfrozen tab gives up the same share of its width.
        if !tabs.shrink_free((asked - room) / asked) {
            break;
        }
        // Tabs that froze at `least` kept more than their share, so the rest start over.
        tabs.restore_free();
    }
    let length = tabs.widths.iter().sum::<f64>() + gaps;
    Strip {
        widths: tabs.widths,
        length,
    }
}

/// Tabs shrinking like flex items: each one's original width, current width, and whether it
/// froze at `least`.
struct Shrinking {
    base: Vec<f64>,
    widths: Vec<f64>,
    frozen: Vec<bool>,
    least: f64,
}

impl Shrinking {
    /// Total width of the tabs frozen at `least`.
    fn frozen_width(&self) -> f64 {
        self.widths
            .iter()
            .zip(&self.frozen)
            .map(|(width, &held)| if held { *width } else { 0.0 })
            .sum::<f64>()
    }

    /// Total width of the unfrozen tabs.
    fn free_width(&self) -> f64 {
        self.base
            .iter()
            .zip(&self.frozen)
            .filter(|(_, &held)| !held)
            .map(|(width, _)| *width)
            .sum()
    }

    /// Shrinks each unfrozen tab by `share` of its width, freezing any that would go below
    /// `least`. Returns whether any froze.
    fn shrink_free(&mut self, share: f64) -> bool {
        let mut froze = false;
        for index in 0..self.base.len() {
            if self.frozen[index] {
                continue;
            }
            let shrunk = self.base[index] * (1.0 - share);
            if shrunk < self.least {
                self.widths[index] = self.least;
                self.frozen[index] = true;
                froze = true;
            } else {
                self.widths[index] = shrunk;
            }
        }
        froze
    }

    /// Restores unfrozen tabs to their original widths.
    fn restore_free(&mut self) {
        for index in 0..self.base.len() {
            if !self.frozen[index] {
                self.widths[index] = self.base[index];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scroll::route::{WHEEL_CLICKS, WHEEL_POINTS};

    const WIDTH: f64 = 1280.0;

    fn shown<'a>(titles: &'a [f64], closes: &'a [bool], active: usize) -> Shown<'a> {
        Shown {
            width: WIDTH,
            titles,
            closes,
            pointer: None,
            active,
            // Tabs keep their places in these rows: a tab's id is its place.
            active_id: Some(TabId(active as u64)),
            update: false,
            sessions: SessionsShown::default(),
            window_buttons: WindowButtons::Shown,
        }
    }

    fn tips<'a>(
        can_go_back: bool,
        can_go_forward: bool,
        update: &'a str,
        sessions: &'a str,
    ) -> Tips<'a> {
        Tips {
            can_go_back,
            can_go_forward,
            update,
            sessions,
        }
    }

    #[test]
    fn the_sessions_button_is_always_there_and_grows_into_a_pill_of_its_words() {
        let titles = [176.0; 12];
        let closes = [false; 12];
        let mut bar = TitleBar::default();
        // With no text it is a bar button 8 pt inside the right end, its glyph centred.
        let quiet = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert_eq!(quiet.sessions, Some(button(WIDTH - 34.0)));
        assert_eq!(
            quiet.sessions_glyph,
            Some(Frame {
                x: WIDTH - 34.0 + 6.0,
                y: 14.0,
                width: 14.0,
                height: 14.0
            })
        );
        assert_eq!(bar.hit(WIDTH - 20.0, 20.0), Control::Sessions);
        assert_eq!(
            bar.tooltip(
                WIDTH - 20.0,
                20.0,
                &tips(false, false, "", "No agents running")
            ),
            Some((button(WIDTH - 34.0), "No agents running".to_owned()))
        );
        // Fully grown around 60 pt of text: 8 + 14 + 6 + 60 + 8 = 96, still ending 8 inside
        // the bar. The tabs give up the 70 it grew by.
        let pill = bar
            .lay_out(&Shown {
                sessions: SessionsShown {
                    words: 60.0,
                    grown: 1.0,
                },
                ..shown(&titles, &closes, 0)
            })
            .clone();
        assert_eq!(
            pill.sessions,
            Some(Frame {
                x: 1272.0 - 96.0,
                y: 8.0,
                width: 96.0,
                height: 26.0
            })
        );
        assert_eq!(pill.sessions_glyph.expect("glyph").x, 1176.0 + 8.0);
        assert_eq!(pill.sessions_words_x, 1176.0 + 8.0 + 14.0 + 6.0);
        let room = |layout: &Layout| layout.tabs_view.expect("tabs").width;
        assert!((room(&quiet) - room(&pill) - 70.0).abs() < 1e-9);
        // Half grown, it and its glyph sit half way between.
        let half = bar
            .lay_out(&Shown {
                sessions: SessionsShown {
                    words: 60.0,
                    grown: 0.5,
                },
                ..shown(&titles, &closes, 0)
            })
            .clone();
        assert_eq!(half.sessions.expect("button").width, 61.0);
        assert_eq!(half.sessions_glyph.expect("glyph").x, 1272.0 - 61.0 + 7.0);
        // With its text gone it shrinks at the last text's width.
        let shrinking = bar
            .lay_out(&Shown {
                sessions: SessionsShown {
                    words: 0.0,
                    grown: 0.5,
                },
                ..shown(&titles, &closes, 0)
            })
            .clone();
        assert_eq!(shrinking.sessions, half.sessions);
        // With an update waiting it sits left of the update button.
        let beside = bar
            .lay_out(&Shown {
                update: true,
                ..shown(&titles, &closes, 0)
            })
            .clone();
        let update = beside.update.expect("update");
        let sessions = beside.sessions.expect("button");
        assert_eq!(sessions.x + sessions.width, update.x - 4.0);
        assert_eq!(sessions.width, 26.0);
    }

    #[test]
    fn tabs_that_fit_keep_their_widths() {
        let laid = strip(&[40.0, 100.0, 300.0], 1000.0, 2.0, 64.0, 176.0);
        assert_eq!(laid.widths, [64.0, 100.0, 176.0]);
        assert_eq!(laid.length, 344.0);
    }

    #[test]
    fn tabs_shrink_in_proportion_before_the_row_scrolls() {
        // Two tabs of 176 and 88 in 200 (198 after the gap): each gives up a quarter.
        let laid = strip(&[176.0, 88.0], 200.0, 2.0, 64.0, 176.0);
        assert!(
            (laid.widths[0] - 132.0).abs() < 1e-9 && (laid.widths[1] - 66.0).abs() < 1e-9,
            "{laid:?}"
        );
        assert!((laid.length - 200.0).abs() < 1e-9);
    }

    #[test]
    fn a_tab_at_its_least_stops_and_the_others_give_up_the_rest() {
        // 176 and 70 in 182 (180 after the gap): in proportion the second would be 51; it
        // stops at 64 and the first takes what is left.
        let laid = strip(&[176.0, 70.0], 182.0, 2.0, 64.0, 176.0);
        assert!(
            (laid.widths[0] - 116.0).abs() < 1e-9 && laid.widths[1] == 64.0,
            "{laid:?}"
        );
        assert!((laid.length - 182.0).abs() < 1e-9);
    }

    #[test]
    fn tabs_all_at_their_least_make_a_row_that_scrolls() {
        let laid = strip(&[176.0; 10], 300.0, 2.0, 64.0, 176.0);
        assert_eq!(laid.widths, [64.0; 10]);
        assert_eq!(laid.length, 658.0);
    }

    #[test]
    fn no_tabs_make_an_empty_row() {
        assert_eq!(
            strip(&[], 300.0, 2.0, 64.0, 176.0),
            Strip {
                widths: vec![],
                length: 0.0
            }
        );
    }

    #[test]
    fn a_tab_asks_for_its_name_and_glyph_and_nothing_for_its_cross() {
        let mut bar = TitleBar::default();
        let layout = bar
            .lay_out(&shown(&[40.0, 60.0], &[false, true], 1))
            .clone();
        // 36 of chrome and the name, whether the tab can close or not.
        assert_eq!(
            layout.tabs[0].frame,
            Frame {
                x: 142.0,
                y: 8.0,
                width: 76.0,
                height: 26.0
            }
        );
        assert_eq!(
            layout.tabs[0].glyph,
            Frame {
                x: 150.0,
                y: 14.0,
                width: 14.0,
                height: 14.0
            }
        );
        assert_eq!(layout.tabs[1].frame.x, 142.0 + 76.0 + 2.0);
        assert_eq!(layout.tabs[1].frame.width, 96.0);
        assert_eq!(layout.tabs[1].label_x, 220.0 + 28.0);
        assert_eq!(layout.tabs[1].fade, None, "a name that fits shows whole");
        // The cross sits inside the tab's right edge, over the end of the name, and is not
        // drawn with the pointer elsewhere.
        assert_eq!(
            layout.tabs[1].close,
            Some(Close {
                frame: Frame {
                    x: 220.0 + 96.0 - 20.0,
                    y: 13.0,
                    width: 16.0,
                    height: 16.0
                },
                shown: false
            })
        );
        assert_eq!(layout.tabs[0].close, None);
        // "+" follows the row after a gap.
        assert_eq!(layout.plus, Some(button(142.0 + 174.0 + 2.0)));
        assert!(!layout.clipped);
    }

    #[test]
    fn the_pointer_on_a_tab_shows_its_cross_over_a_name_that_fades_out_before_it() {
        let titles = [40.0, 60.0];
        let closes = [true, true];
        let mut bar = TitleBar::default();
        let rested = bar.lay_out(&shown(&titles, &closes, 1)).clone();
        // On the second tab, selected, which runs from 220 to 316.
        let pointed = bar
            .lay_out(&Shown {
                pointer: Some((230.0, 20.0)),
                ..shown(&titles, &closes, 1)
            })
            .clone();
        let frames = |layout: &Layout| -> Vec<(Frame, f64)> {
            layout
                .tabs
                .iter()
                .map(|tab| (tab.frame, tab.label_x))
                .collect()
        };
        assert_eq!(
            frames(&rested),
            frames(&pointed),
            "the cross moves no tab and no name"
        );
        assert_eq!(pointed.plus, rested.plus);
        let cross = pointed.tabs[1].close.expect("it can close");
        assert!(cross.shown);
        assert_eq!(cross.frame, rested.tabs[1].close.expect("a cross").frame);
        // The name runs from 248 to 308; the cross starts at 296. Nothing of the name is
        // left 2 before it, after a fade of 16.
        assert_eq!(
            pointed.tabs[1].fade,
            Some(Fade {
                start: 278.0,
                end: 294.0
            })
        );
        // The other tab, selected or not, shows no cross and its whole name.
        assert_eq!(pointed.tabs[0].close.map(|close| close.shown), Some(false));
        assert_eq!(pointed.tabs[0].fade, None);
        let other = bar
            .lay_out(&Shown {
                pointer: Some((150.0, 20.0)),
                ..shown(&titles, &closes, 1)
            })
            .clone();
        assert_eq!(
            (
                other.tabs[0].close.map(|close| close.shown),
                other.tabs[1].close.map(|close| close.shown)
            ),
            (Some(true), Some(false)),
            "a tab that is not selected shows its cross under the pointer too"
        );
    }

    #[test]
    fn a_tab_that_cannot_close_shows_no_cross_under_the_pointer() {
        let mut bar = TitleBar::default();
        let layout = bar
            .lay_out(&Shown {
                pointer: Some((150.0, 20.0)),
                ..shown(&[40.0], &[false], 0)
            })
            .clone();
        assert_eq!((layout.tabs[0].close, layout.tabs[0].fade), (None, None));
        assert_eq!(bar.hit(204.0, 20.0), Control::Tab(0));
    }

    #[test]
    fn a_name_too_long_for_its_tab_fades_out_at_the_tabs_padding() {
        // A tab is at most 176 wide: a name of 200 has 140 of it, from 170 to 310.
        let mut bar = TitleBar::default();
        let rested = bar
            .lay_out(&shown(&[200.0, 40.0], &[true, true], 0))
            .clone();
        assert_eq!(rested.tabs[0].frame.width, 176.0);
        assert_eq!(
            rested.tabs[0].fade,
            Some(Fade {
                start: 294.0,
                end: 310.0
            })
        );
        // On hover it is gone 2 before the cross, which starts at 298.
        let pointed = bar
            .lay_out(&Shown {
                pointer: Some((150.0, 20.0)),
                ..shown(&[200.0, 40.0], &[true, true], 0)
            })
            .clone();
        assert_eq!(
            pointed.tabs[0].fade,
            Some(Fade {
                start: 280.0,
                end: 296.0
            })
        );
    }

    #[test]
    fn a_narrow_tabs_name_fades_over_no_more_than_half_its_room() {
        // Twenty tabs at their least, 64: a name has 28 of it, and 14 under the pointer.
        let titles = [176.0; 20];
        let closes = [true; 20];
        let mut bar = TitleBar::default();
        let rested = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert_eq!(
            rested.tabs[0].fade,
            Some(Fade {
                start: 184.0,
                end: 198.0
            })
        );
        let pointed = bar
            .lay_out(&Shown {
                pointer: Some((150.0, 20.0)),
                ..shown(&titles, &closes, 0)
            })
            .clone();
        assert_eq!(pointed.tabs[0].frame.width, 64.0);
        assert_eq!(
            pointed.tabs[0].fade,
            Some(Fade {
                start: 177.0,
                end: 184.0
            })
        );
        assert_eq!(bar.hit(190.0, 20.0), Control::Close(0));
        assert_eq!(bar.hit(180.0, 20.0), Control::Tab(0));
    }

    #[test]
    fn a_pointer_outside_the_rows_view_is_on_no_tab() {
        let titles = [176.0; 20];
        let closes = [true; 20];
        let mut bar = TitleBar::default();
        // Scrolled 250, the fourth tab runs from 90 to 154: at 139 it is under Forward's side
        // of the bar, out of the row's view.
        let layout = bar
            .lay_out(&Shown {
                pointer: Some((139.0, 20.0)),
                ..shown(&titles, &closes, 19)
            })
            .clone();
        assert_eq!(layout.tabs[3].frame.x, 90.0);
        assert!(layout
            .tabs
            .iter()
            .all(|tab| !tab.close.expect("a cross").shown));
    }

    #[test]
    fn without_the_window_buttons_back_forward_and_the_tabs_start_at_the_bars_edge() {
        let titles = [40.0, 40.0, 40.0];
        let closes = [false; 3];
        let mut bar = TitleBar::default();
        // Beside the window's own buttons, Back starts 86 in.
        let beside = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert_eq!(
            (beside.back, beside.forward),
            (Some(button(86.0)), Some(button(112.0)))
        );
        assert_eq!(beside.tabs[0].frame.x, 142.0);
        // In full screen they are hidden: Back is 8 in, matching the right-end inset, and
        // everything after it moves left by 78.
        let hidden = Shown {
            window_buttons: WindowButtons::Hidden,
            ..shown(&titles, &closes, 0)
        };
        let full = bar.lay_out(&hidden).clone();
        assert_eq!(
            (full.back, full.forward),
            (Some(button(8.0)), Some(button(34.0)))
        );
        assert_eq!(full.tabs_view.map(|view| view.x), Some(64.0));
        let lefts: Vec<f64> = full.tabs.iter().map(|tab| tab.frame.x).collect();
        assert_eq!(lefts, [64.0, 142.0, 220.0]);
        assert_eq!(full.plus, Some(button(64.0 + 232.0 + 2.0)));
        assert_eq!(full.sessions, beside.sessions, "the right end stays");
        assert_eq!(bar.hit(10.0, 20.0), Control::Back);
        assert_eq!(bar.hit(70.0, 20.0), Control::Tab(0));
        // A dragged tab takes its places from the row's new start.
        bar.hold(0, 84.0, 20.0);
        assert_eq!(bar.carry(144.0, 20.0, 2.0), Carry::Moved(0, 1));
        assert_eq!(bar.lay_out(&hidden).tabs[1].frame.x, 124.0);
        assert!(bar.let_go());
    }

    #[test]
    fn the_update_button_takes_room_from_the_tabs() {
        let titles = [176.0; 12];
        let closes = [false; 12];
        let mut bar = TitleBar::default();
        let without = bar
            .lay_out(&shown(&titles, &closes, 0))
            .tabs_view
            .expect("tabs")
            .width;
        let with = bar
            .lay_out(&Shown {
                update: true,
                ..shown(&titles, &closes, 0)
            })
            .clone();
        // 1280 less 8 + 26 for the sessions button and 8 + 26 + 2 + 142, then 30 more for the
        // update button and its 4 pt gap; the tabs shrink to fit.
        assert!((without - 1068.0).abs() < 1e-9, "{without}");
        assert!((with.tabs_view.expect("tabs").width - 1038.0).abs() < 1e-9);
        assert_eq!(with.update, Some(button(WIDTH - 34.0)));
        assert_eq!(with.sessions, Some(button(WIDTH - 34.0 - 30.0)));
    }

    #[test]
    fn a_tab_just_chosen_is_scrolled_into_view_and_a_scroll_is_held_to_the_row() {
        let titles = [176.0; 20];
        let closes = [false; 20];
        let mut bar = TitleBar::default();
        // Twenty tabs at their least: 20 × 64 and 19 gaps, more than the 1068 the bar holds.
        let first = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert!(first.clipped);
        assert_eq!((first.length, first.offset), (1318.0, 0.0));
        let last = bar.lay_out(&shown(&titles, &closes, 19)).clone();
        assert_eq!(last.offset, 1318.0 - 1068.0);
        assert_eq!(last.tabs[19].frame.x + 64.0, 142.0 + 1068.0);
        // Redrawn with no change, it stays put; a scroll stops at either end.
        assert_eq!(bar.lay_out(&shown(&titles, &closes, 19)).offset, 250.0);
        assert!(bar.scroll(-100.0, WHEEL_POINTS));
        assert_eq!(bar.layout().offset, 150.0);
        assert!(bar.scroll(-400.0, WHEEL_POINTS));
        assert_eq!(bar.layout().offset, 0.0);
        assert!(!bar.scroll(-10.0, WHEEL_POINTS));
        assert!(bar.scroll(1000.0, WHEEL_POINTS));
        assert_eq!(bar.layout().offset, 250.0);
        // A wheel's click with ⇧ held goes 40 points.
        assert!(bar.scroll(-1.0, WHEEL_CLICKS));
        assert_eq!(bar.layout().offset, 210.0);
    }

    #[test]
    fn closing_another_tab_leaves_the_row_where_it_is_scrolled() {
        let titles = [176.0; 20];
        let closes = [true; 20];
        let mut bar = TitleBar::default();
        // The last of twenty tabs is active and in view; the row is then wheeled to its start.
        bar.lay_out(&shown(&titles, &closes, 19));
        assert!(bar.scroll(-1000.0, WHEEL_POINTS));
        assert_eq!(bar.layout().offset, 0.0);
        // The third tab closes: the active tab is the same one, now the nineteenth of
        // nineteen, and the row stays at its start.
        let closed = bar
            .lay_out(&Shown {
                active_id: Some(TabId(19)),
                ..shown(&titles[..19], &closes[..19], 18)
            })
            .clone();
        assert_eq!(closed.offset, 0.0);
        assert_eq!(closed.tabs[2].frame.x, 142.0 + 2.0 * 66.0);
        // The active tab closes: its neighbour is active now, and is brought into view.
        let next = bar
            .lay_out(&Shown {
                active_id: Some(TabId(18)),
                ..shown(&titles[..18], &closes[..18], 17)
            })
            .clone();
        assert_eq!(next.offset, next.length - 1068.0);
    }

    #[test]
    fn a_row_scrolled_past_its_end_by_a_tab_closing_is_held_to_its_end() {
        let titles = [176.0; 20];
        let closes = [true; 20];
        let mut bar = TitleBar::default();
        // The first tab is active; the row is wheeled to its end, 250 along.
        bar.lay_out(&shown(&titles, &closes, 0));
        assert!(bar.scroll(1000.0, WHEEL_POINTS));
        assert_eq!(bar.layout().offset, 250.0);
        // The last tab closes: the row is 66 shorter and scrolls back by that much.
        let closed = bar.lay_out(&shown(&titles[..19], &closes[..19], 0)).clone();
        assert_eq!(closed.offset, 184.0);
    }

    #[test]
    fn a_scroll_is_measured_against_the_rows_own_length() {
        // The row's length is the strip's, gaps included, whatever is drawn of it.
        let titles = [176.0; 20];
        let closes = [false; 20];
        let mut bar = TitleBar::default();
        bar.lay_out(&shown(&titles, &closes, 0));
        bar.scroll(10_000.0, WHEEL_POINTS);
        let layout = bar.layout();
        assert_eq!(
            layout.offset,
            layout.length - layout.tabs_view.expect("tabs").width
        );
    }

    #[test]
    fn a_point_is_on_one_control() {
        let mut bar = TitleBar::default();
        bar.lay_out(&Shown {
            update: true,
            ..shown(&[40.0, 60.0], &[false, true], 1)
        });
        assert_eq!(bar.hit(90.0, 20.0), Control::Back);
        assert_eq!(bar.hit(120.0, 20.0), Control::Forward);
        assert_eq!(bar.hit(150.0, 20.0), Control::Tab(0));
        assert_eq!(bar.hit(230.0, 20.0), Control::Tab(1));
        assert_eq!(bar.hit(300.0, 20.0), Control::Close(1));
        assert_eq!(bar.hit(325.0, 20.0), Control::Plus);
        assert_eq!(bar.hit(WIDTH - 20.0, 20.0), Control::Update);
        assert_eq!(bar.hit(WIDTH - 50.0, 20.0), Control::Sessions);
        assert_eq!(bar.hit(600.0, 20.0), Control::Bar);
        assert_eq!(
            bar.hit(150.0, 2.0),
            Control::Bar,
            "above the tab, on the bar"
        );
        assert_eq!(
            (bar.tab_at(150.0, 20.0), bar.tab_at(300.0, 20.0)),
            (Some(0), Some(1)),
            "a tab's cross is the tab's"
        );
        assert_eq!(
            (bar.tab_at(325.0, 20.0), bar.tab_at(600.0, 20.0)),
            (None, None)
        );
    }

    #[test]
    fn a_tab_scrolled_out_of_the_row_takes_no_press() {
        let titles = [176.0; 20];
        let closes = [false; 20];
        let mut bar = TitleBar::default();
        bar.lay_out(&shown(&titles, &closes, 19));
        // Scrolled 250, the fourth tab runs from 90 to 154: only its part inside the row's
        // view, from 142, is a tab.
        assert_eq!(bar.layout().tabs[3].frame.x, 90.0);
        assert_eq!(bar.hit(139.0, 20.0), Control::Bar);
        assert_eq!(bar.hit(150.0, 20.0), Control::Tab(3));
    }

    #[test]
    fn a_rested_pointer_names_what_is_open() {
        let mut bar = TitleBar::default();
        bar.lay_out(&shown(&[40.0], &[false], 0));
        assert_eq!(
            bar.tooltip(345.0 - 120.0, 20.0, &tips(false, false, "", "")),
            Some((button(142.0 + 76.0 + 2.0), NEW_TAB.to_owned()))
        );
        assert_eq!(
            bar.tooltip(90.0, 20.0, &tips(false, true, "", "")),
            None,
            "Back goes nowhere"
        );
        assert_eq!(
            bar.tooltip(90.0, 20.0, &tips(true, false, "", ""))
                .map(|(_, words)| words),
            Some(BACK.to_owned())
        );
        assert_eq!(
            bar.tooltip(120.0, 20.0, &tips(true, true, "", ""))
                .map(|(_, words)| words),
            Some(FORWARD.to_owned())
        );
        assert_eq!(
            bar.tooltip(150.0, 20.0, &tips(true, true, "", "")),
            None,
            "a tab says nothing"
        );
    }

    #[test]
    fn a_carried_tab_goes_to_the_place_nearest_its_left_edge() {
        // Others 100 and 60 wide from 0 with gaps of 2: places start at 0, 102 and 164.
        assert_eq!(nearest_place(&[100.0, 60.0], 0.0, 2.0, -30.0), 0);
        assert_eq!(
            nearest_place(&[100.0, 60.0], 0.0, 2.0, 50.0),
            0,
            "a tie keeps the first"
        );
        assert_eq!(nearest_place(&[100.0, 60.0], 0.0, 2.0, 52.0), 1);
        assert_eq!(nearest_place(&[100.0, 60.0], 0.0, 2.0, 140.0), 2);
        assert_eq!(nearest_place(&[], 0.0, 2.0, 140.0), 0);
    }

    #[test]
    fn a_held_tab_is_carried_only_past_a_drags_start() {
        let titles = [40.0, 40.0, 40.0];
        let closes = [false; 3];
        let mut bar = TitleBar::default();
        bar.lay_out(&shown(&titles, &closes, 0));
        // Three tabs 76 wide from 142: at 142, 220 and 298. The first is pressed 20 in.
        bar.hold(0, 162.0, 20.0);
        assert_eq!(
            bar.carry(163.0, 20.0, 2.0),
            Carry::Still,
            "1 point is no drag"
        );
        assert_eq!(bar.carried(), None);
        assert_eq!(bar.carry(163.5, 20.0, 2.0), Carry::Carried, "1.5 points is");
        assert_eq!(bar.carried(), Some(0));
        let laid = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert_eq!(laid.tabs[0].frame.x, 143.5, "it follows the pointer");
        assert_eq!(laid.tabs[0].label_x, 143.5 + 28.0);
        assert_eq!(laid.tabs[1].frame.x, 220.0, "the others stay");
        assert!(bar.let_go());
        assert!(!bar.let_go(), "nothing is held after");
        assert_eq!(bar.carry(300.0, 20.0, 2.0), Carry::Still);
    }

    #[test]
    fn a_carried_tab_takes_the_others_places_as_it_passes_them() {
        let titles = [40.0, 40.0, 40.0];
        let closes = [false; 3];
        let mut bar = TitleBar::default();
        bar.lay_out(&shown(&titles, &closes, 0));
        bar.hold(0, 162.0, 20.0);
        // Its left edge at 180 is nearer the first place (142) than the second (220); at 182,
        // the second.
        assert_eq!(bar.carry(200.0, 20.0, 2.0), Carry::Carried);
        assert_eq!(bar.carry(202.0, 20.0, 2.0), Carry::Moved(0, 1));
        // Past the third before the next layout: counted from the new order.
        assert_eq!(bar.carry(300.0, 20.0, 2.0), Carry::Moved(1, 2));
        let laid = bar.lay_out(&shown(&titles, &closes, 2)).clone();
        assert_eq!(bar.carried(), Some(2));
        assert_eq!((laid.tabs[0].frame.x, laid.tabs[1].frame.x), (142.0, 220.0));
        assert_eq!(laid.tabs[2].frame.x, 280.0, "drawn under the pointer");
        // It is clamped to the row's end and start.
        bar.carry(900.0, 20.0, 2.0);
        assert_eq!(
            bar.lay_out(&shown(&titles, &closes, 2)).tabs[2].frame.x,
            298.0
        );
        assert_eq!(bar.carry(0.0, 20.0, 2.0), Carry::Moved(2, 0));
        assert_eq!(
            bar.lay_out(&shown(&titles, &closes, 0)).tabs[0].frame.x,
            142.0
        );
        assert!(bar.let_go());
        let laid = bar.lay_out(&shown(&titles, &closes, 0)).clone();
        assert_eq!(laid.tabs[0].frame.x, 142.0, "it settles in its place");
    }
}
