//! Animation durations and curves, kept in one place so every shell animates the same way. The
//! values come from `tw-animate-css`, Tailwind's defaults, and Radix's tooltip and menu
//! primitives.
//!
//! A menu, popover or tooltip enters over 150 ms on CSS `ease`: fading in, scaling from 95% about
//! its origin, and sliding 8 pt in from the side it opens on. It exits the same way without the
//! slide. A dialog panel takes 200 ms and does not slide; the scrim behind it takes 150 ms. Hover
//! colour changes take 150 ms on Tailwind's standard curve.

/// A CSS timing function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Curve {
    /// `ease`: cubic-bezier(0.25, 0.1, 0.25, 1), used for every entrance and exit.
    Ease,
    /// Tailwind's default for transitions: cubic-bezier(0.4, 0, 0.2, 1).
    Standard,
    Linear,
    /// `ease-in`: cubic-bezier(0.42, 0, 1, 1).
    EaseIn,
    /// `ease-out`: cubic-bezier(0, 0, 0.58, 1).
    EaseOut,
}

impl Curve {
    fn points(self) -> (f64, f64, f64, f64) {
        match self {
            Curve::Ease => (0.25, 0.1, 0.25, 1.0),
            Curve::Standard => (0.4, 0.0, 0.2, 1.0),
            Curve::Linear => (0.0, 0.0, 1.0, 1.0),
            Curve::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Curve::EaseOut => (0.0, 0.0, 0.58, 1.0),
        }
    }

    /// The eased progress at normalized `time` (0 to 1).
    pub fn at(self, time: f64) -> f64 {
        let time = time.clamp(0.0, 1.0);
        if self == Curve::Linear || time == 0.0 || time == 1.0 {
            return time;
        }
        let (x1, y1, x2, y2) = self.points();
        let bezier = |a: f64, b: f64, t: f64| {
            let u = 1.0 - t;
            3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
        };
        // The curve's x increases with its parameter, so bisect to find the parameter for a time.
        let (mut low, mut high) = (0.0_f64, 1.0_f64);
        for _ in 0..40 {
            let middle = (low + high) / 2.0;
            if bezier(x1, x2, middle) < time {
                low = middle;
            } else {
                high = middle;
            }
        }
        bezier(y1, y2, (low + high) / 2.0)
    }
}

/// Duration of a menu, popover, select or tooltip entrance or exit.
pub const POPUP_MS: f64 = 150.0;
/// Duration of a dialog panel's entrance or exit.
pub const DIALOG_MS: f64 = 200.0;
/// Duration of the scrim behind a dialog.
pub const SHADE_MS: f64 = 150.0;
/// Duration of a hover or switch colour or position change.
pub const HOVER_MS: f64 = 150.0;
/// How long the pointer rests on a control before its tooltip shows.
pub const TOOLTIP_DELAY_MS: f64 = 200.0;
/// After a tooltip shows, the next one within this window shows at once.
pub const TOOLTIP_SKIP_MS: f64 = 300.0;
/// How long the pointer rests on a menu row before its submenu opens.
pub const SUBMENU_DELAY_MS: f64 = 100.0;
/// How far a popup slides in from the side it opens on, in points.
pub const SLIDE: f64 = 8.0;
/// The scale a popup or dialog grows from and shrinks to.
pub const SMALL: f64 = 0.95;
/// The redraw interval for animation: every frame at 60 Hz. The busy spinner uses it.
pub const FRAME_SECONDS: f64 = 1.0 / 60.0;
/// The redraw interval for popup, dialog and hover animations: 120 Hz, so a 120 Hz screen shows
/// every step.
pub const MOVE_FRAME_SECONDS: f64 = 1.0 / 120.0;
/// How often a player's controls update to follow the playing clip.
pub const FOLLOW_SECONDS: f64 = 1.0 / 30.0;
/// How long the board's zoom label takes to fade in or out.
pub const ZOOM_HUD_FADE_MS: f64 = 150.0;
/// How long the zoom label stays after the last zoom step.
pub const ZOOM_HUD_HOLD_MS: f64 = 900.0;
/// How far, in points, the zoom label drops as it appears and rises as it leaves.
pub const ZOOM_HUD_RISE: f64 = 4.0;

/// The animated state of an entering or exiting element at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub opacity: f64,
    pub scale: f64,
    /// The remaining slide: 1 at the start of an entrance, 0 at rest.
    pub slide: f64,
}

/// The pose `elapsed` milliseconds into an entrance of `duration` milliseconds.
pub fn enter(elapsed: f64, duration: f64) -> Pose {
    let along = Curve::Ease.at(if duration > 0.0 {
        elapsed / duration
    } else {
        1.0
    });
    Pose {
        opacity: along,
        scale: SMALL + (1.0 - SMALL) * along,
        slide: 1.0 - along,
    }
}

/// The pose `elapsed` milliseconds into an exit of `duration` milliseconds. Exits do not slide.
pub fn exit(elapsed: f64, duration: f64) -> Pose {
    let along = Curve::Ease.at(if duration > 0.0 {
        elapsed / duration
    } else {
        1.0
    });
    Pose {
        opacity: 1.0 - along,
        scale: 1.0 - (1.0 - SMALL) * along,
        slide: 0.0,
    }
}

/// The time at which a check freezes animation (`launch::Launch::held_ms`): `NR_MOTION_AT` for
/// entrances and hover changes, `NR_MOTION_EXIT_AT` for exits.
pub type Held = Option<f64>;

/// Milliseconds since an animation began at `since` (seconds on the platform clock, now at
/// `now`), or the frozen time a check sets.
pub fn elapsed(since: f64, now: f64, held: Held) -> f64 {
    held.unwrap_or((now - since) * 1000.0)
}

/// A popup or dialog animation's pose `elapsed` milliseconds in, and whether it is finished. An
/// animation a check freezes never finishes; it stays as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub pose: Pose,
    pub done: bool,
}

pub fn frame(elapsed: f64, duration: f64, leaving: bool, held: Held) -> Frame {
    let pose = if leaving {
        exit(elapsed, duration)
    } else {
        enter(elapsed, duration)
    };
    Frame {
        pose,
        done: held.is_none() && elapsed >= duration,
    }
}

/// A button's hover level (0 to 1) `elapsed` milliseconds into a background change (`on`, or
/// off after the pointer left) that started from `from`.
pub fn hover_level(from: f64, on: bool, elapsed: f64) -> f64 {
    let eased = Curve::Standard.at(elapsed / HOVER_MS).clamp(0.0, 1.0);
    let target = if on { 1.0 } else { 0.0 };
    from + (target - from) * eased
}

/// Whether a hover change `elapsed` milliseconds in is finished. A frozen one is drawn once, at
/// the frozen time.
pub fn hover_settled(elapsed: f64, held: Held) -> bool {
    held.is_some() || elapsed >= HOVER_MS
}

/// The busy spinner's rotation as a fraction of a turn: one turn per second, like Tailwind's
/// `animate-spin`, from the platform clock at `seconds`. It stays still under reduced motion,
/// and uses the frozen time when a check holds time.
pub fn spin(seconds: f64, reduced: bool, held: Held) -> f64 {
    match held {
        Some(held) => (held / 1000.0) % 1.0,
        None if reduced => 0.0,
        None => seconds % 1.0,
    }
}

/// Whether the busy spinner animates with the clock (`spin`) and so needs a redraw every frame;
/// not under reduced motion or when a check holds time.
pub fn spins(reduced: bool, held: Held) -> bool {
    held.is_none() && !reduced
}

/// Hover background fades for buttons (Tailwind's `transition-colors`), keyed by whatever the
/// shell names each button. A reversal starts from wherever the last change had reached.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HoverFades {
    changes: std::collections::HashMap<String, HoverChange>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct HoverChange {
    /// When it began, in seconds on the platform's clock.
    since: f64,
    on: bool,
    from: f64,
}

/// The change time given to a button first seen unlit, far enough back that it is settled.
const UNSEEN_SINCE: f64 = -1000.0;

/// A button's current hover level, and whether a change began, which animates until settled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverLevel {
    pub level: f64,
    pub began: bool,
}

impl HoverFades {
    /// The hover level (0 to 1) of `key` at `now` (seconds on the platform clock), with the pointer
    /// over it (`on`) or not. A newly seen button starts unlit unless the pointer is already on it,
    /// which starts its fade.
    pub fn level(&mut self, key: &str, on: bool, now: f64, held: Held) -> HoverLevel {
        let began = match self.changes.get(key) {
            None => {
                self.changes.insert(
                    key.to_owned(),
                    HoverChange {
                        since: if on { now } else { UNSEEN_SINCE },
                        on,
                        from: 0.0,
                    },
                );
                on
            }
            Some(change) if change.on != on => {
                let from = Self::at(change, now, held);
                self.changes.insert(
                    key.to_owned(),
                    HoverChange {
                        since: now,
                        on,
                        from,
                    },
                );
                true
            }
            Some(_) => false,
        };
        HoverLevel {
            level: Self::at(&self.changes[key], now, held),
            began,
        }
    }

    /// Whether every fade has finished at `now`, so nothing needs redrawing.
    pub fn settled(&self, now: f64, held: Held) -> bool {
        self.changes
            .values()
            .all(|change| hover_settled(elapsed(change.since, now, None), held))
    }

    fn at(change: &HoverChange, now: f64, held: Held) -> f64 {
        hover_level(change.from, change.on, elapsed(change.since, now, held))
    }
}

/// The single on-screen tooltip, as in Radix's `TooltipProvider`. Resting the pointer on a
/// control shows its tooltip after `TOOLTIP_DELAY_MS`, or at once within `TOOLTIP_SKIP_MS` of
/// the last one showing or hiding. Times are milliseconds on the platform's monotonic clock; the
/// shell keys each control however it likes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tooltip {
    /// The control the pointer rests on.
    resting: Option<String>,
    /// Whether its tooltip is showing.
    showing: bool,
    /// When a tooltip last showed or hid.
    last_ms: Option<f64>,
}

impl Tooltip {
    /// The pointer came to rest on `key` at `now_ms`. `None` when it was already resting there.
    /// Otherwise any showing tooltip hides (`leave`) and the result is how many milliseconds until
    /// `key`'s tooltip is due.
    pub fn rest(&mut self, key: &str, now_ms: f64) -> Option<f64> {
        if self.resting.as_deref() == Some(key) {
            return None;
        }
        self.leave(now_ms);
        self.resting = Some(key.to_owned());
        let recent = self
            .last_ms
            .is_some_and(|last| now_ms - last < TOOLTIP_SKIP_MS);
        Some(if recent { 0.0 } else { TOOLTIP_DELAY_MS })
    }

    /// Called when `key`'s delay ends; returns whether its tooltip shows now, which happens only
    /// while the pointer still rests on `key` and the tooltip is not already showing.
    pub fn due(&mut self, key: &str, now_ms: f64) -> bool {
        if self.resting.as_deref() != Some(key) || self.showing {
            return false;
        }
        self.showing = true;
        self.last_ms = Some(now_ms);
        true
    }

    /// The pointer left or pressed the control. Returns whether a tooltip was showing, which the
    /// shell then hides.
    pub fn leave(&mut self, now_ms: f64) -> bool {
        self.resting = None;
        if !self.showing {
            return false;
        }
        self.showing = false;
        self.last_ms = Some(now_ms);
        true
    }
}

/// A tooltip body: 28 pt tall, text 12 pt from either side and 6 pt down, 6 px corner radius.
pub const TOOLTIP_HEIGHT: f64 = 28.0;
/// The tooltip text's line height.
pub const TOOLTIP_LINE: f64 = 16.0;
const TOOLTIP_PADDING_X: f64 = 12.0;
const TOOLTIP_PADDING_Y: f64 = 6.0;
/// The gap between the control and the body: the 5 pt arrow (Radix's `arrowHeight`) plus the
/// 5 pt side offset.
const TOOLTIP_GAP: f64 = 10.0;

/// A tooltip's layout, in points from the top-left of its control's container.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipPlace {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// Where its text starts and its maximum width.
    pub text_x: f64,
    pub text_y: f64,
    pub text_width: f64,
    /// The arrow's x from the body's left, at the control's centre.
    pub pointer: f64,
}

/// The tooltip below a control at `x`, `y`, `width` by `height`, with text `words` wide: centred
/// on the control and kept within `bounds_width`.
pub fn place_tooltip(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    words: f64,
    bounds_width: f64,
) -> TooltipPlace {
    let body_width = (words + 2.0 * TOOLTIP_PADDING_X).ceil();
    let middle = x + width / 2.0;
    let left = (middle - body_width / 2.0)
        .max(0.0)
        .min(bounds_width - body_width);
    let top = y + height + TOOLTIP_GAP;
    TooltipPlace {
        x: left,
        y: top,
        width: body_width,
        height: TOOLTIP_HEIGHT,
        text_x: left + TOOLTIP_PADDING_X,
        text_y: top + TOOLTIP_PADDING_Y,
        text_width: body_width - 2.0 * TOOLTIP_PADDING_X,
        pointer: middle - left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hover_fades_in_and_turns_back_from_where_it_got_to() {
        let mut fades = HoverFades::default();
        // First seen unlit: at rest, nothing to draw.
        assert_eq!(
            fades.level("back", false, 10.0, None),
            HoverLevel {
                level: 0.0,
                began: false
            }
        );
        assert!(fades.settled(10.0, None));
        // The pointer arrives: the fade begins and finishes after `HOVER_MS`.
        let lit = fades.level("back", true, 10.0, None);
        assert_eq!(
            lit,
            HoverLevel {
                level: 0.0,
                began: true
            }
        );
        assert!(!fades.settled(10.05, None));
        let partway = fades.level("back", true, 10.05, None).level;
        assert!(partway > 0.0 && partway < 1.0);
        // It leaves part-way: the fade out starts from there, not from fully lit.
        let leaving = fades.level("back", false, 10.05, None);
        assert_eq!(
            leaving,
            HoverLevel {
                level: partway,
                began: true
            }
        );
        assert_eq!(fades.level("back", false, 11.0, None).level, 0.0);
        assert!(fades.settled(11.0, None));
    }

    #[test]
    fn a_button_first_seen_under_the_pointer_starts_lighting() {
        let mut fades = HoverFades::default();
        assert!(fades.level("plus", true, 5.0, None).began);
        assert_eq!(fades.level("plus", true, 6.0, None).level, 1.0);
        // A check holding time draws it once, at the frozen time.
        let mut held = HoverFades::default();
        held.level("plus", true, 5.0, Some(HOVER_MS));
        assert!(held.settled(5.0, Some(HOVER_MS)));
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 5e-4
    }

    /// The curves' values as a browser renders them: read from a tooltip's opacity paused at these
    /// points in its entrance (`ease`), and from the CSS definition for the standard curve.
    #[test]
    fn the_curves_are_the_browsers() {
        assert!(near(Curve::Ease.at(0.25), 0.4085));
        assert!(near(Curve::Ease.at(0.5), 0.8024));
        assert!(near(Curve::Ease.at(0.75), 0.9604));
        assert!(near(Curve::Standard.at(0.5), 0.7756));
        assert!(near(Curve::Standard.at(0.25), 0.2366));
        assert_eq!(Curve::Linear.at(0.3), 0.3);
        assert_eq!(Curve::Ease.at(-1.0), 0.0);
        assert_eq!(Curve::Ease.at(2.0), 1.0);
    }

    #[test]
    fn an_entrance_grows_fades_in_and_slides_to_rest() {
        assert_eq!(
            enter(0.0, POPUP_MS),
            Pose {
                opacity: 0.0,
                scale: 0.95,
                slide: 1.0
            }
        );
        let half = enter(75.0, POPUP_MS);
        assert!(
            near(half.opacity, 0.8024)
                && near(half.scale, 0.95 + 0.05 * 0.8024)
                && near(half.slide, 0.1976)
        );
        assert_eq!(
            enter(150.0, POPUP_MS),
            Pose {
                opacity: 1.0,
                scale: 1.0,
                slide: 0.0
            }
        );
        assert_eq!(enter(900.0, POPUP_MS), enter(150.0, POPUP_MS));
        assert_eq!(
            enter(0.0, 0.0),
            Pose {
                opacity: 1.0,
                scale: 1.0,
                slide: 0.0
            }
        );
    }

    #[test]
    fn an_exit_shrinks_and_fades_without_sliding() {
        assert_eq!(
            exit(0.0, DIALOG_MS),
            Pose {
                opacity: 1.0,
                scale: 1.0,
                slide: 0.0
            }
        );
        let half = exit(100.0, DIALOG_MS);
        assert!(
            near(half.opacity, 0.1976)
                && near(half.scale, 1.0 - 0.05 * 0.8024)
                && half.slide == 0.0
        );
        assert!(
            near(exit(200.0, DIALOG_MS).opacity, 0.0) && near(exit(200.0, DIALOG_MS).scale, 0.95)
        );
    }

    #[test]
    fn a_check_holds_a_move_where_it_says() {
        assert_eq!(elapsed(10.0, 10.25, None), 250.0);
        assert_eq!(elapsed(10.0, 99.0, Some(75.0)), 75.0);
        let running = frame(75.0, POPUP_MS, false, None);
        assert_eq!((running.pose, running.done), (enter(75.0, POPUP_MS), false));
        assert!(frame(150.0, POPUP_MS, true, None).done);
        assert!(
            !frame(900.0, POPUP_MS, false, Some(900.0)).done,
            "a held move stays as drawn"
        );
        assert_eq!(
            frame(100.0, DIALOG_MS, true, None).pose,
            exit(100.0, DIALOG_MS)
        );
    }

    #[test]
    fn a_hover_eases_from_where_it_was_to_lit_or_dark() {
        assert_eq!(hover_level(0.0, true, 0.0), 0.0);
        assert!(near(hover_level(0.0, true, 75.0), 0.7756));
        assert_eq!(hover_level(0.0, true, 400.0), 1.0);
        assert!(near(hover_level(0.6, false, 75.0), 0.6 - 0.6 * 0.7756));
        assert!(
            !hover_settled(149.0, None)
                && hover_settled(150.0, None)
                && hover_settled(0.0, Some(20.0))
        );
    }

    #[test]
    fn a_tooltip_waits_then_the_next_within_the_skip_window_shows_at_once() {
        let mut tip = Tooltip::default();
        assert_eq!(tip.rest("back", 1000.0), Some(TOOLTIP_DELAY_MS));
        assert_eq!(
            tip.rest("back", 1100.0),
            None,
            "resting on the same control changes nothing"
        );
        assert!(tip.due("back", 1200.0));
        assert!(!tip.due("back", 1210.0), "shown once");
        // Straight on to the next control: it shows at once, and the first hides.
        assert_eq!(tip.rest("tab", 1400.0), Some(0.0));
        assert!(!tip.due("back", 1400.0), "the first is no longer rested on");
        assert!(tip.due("tab", 1400.0));
        assert!(tip.leave(1500.0));
        assert!(!tip.leave(1510.0), "nothing is showing");
        assert_eq!(
            tip.rest("account", 1799.0),
            Some(0.0),
            "within the skip window of the last going"
        );
        assert!(!tip.leave(1850.0), "left before it was due");
        assert!(!tip.due("account", 1900.0));
        assert_eq!(
            tip.rest("account", 1900.0),
            Some(TOOLTIP_DELAY_MS),
            "the skip window is over"
        );
    }

    #[test]
    fn a_tooltip_sits_10_under_its_control_kept_inside() {
        let under = place_tooltip(100.0, 10.0, 28.0, 28.0, 40.3, 1280.0);
        assert_eq!(
            (under.x, under.y, under.width, under.height),
            (114.0 - 32.5, 48.0, 65.0, 28.0)
        );
        assert_eq!(
            (under.text_x, under.text_y, under.text_width),
            (93.5, 54.0, 41.0)
        );
        assert_eq!(under.pointer, 32.5);
        // Kept inside the space: at the left edge, and at the right.
        let left = place_tooltip(0.0, 0.0, 20.0, 20.0, 76.0, 1280.0);
        assert_eq!((left.x, left.pointer), (0.0, 10.0));
        let right = place_tooltip(1260.0, 0.0, 20.0, 20.0, 76.0, 1280.0);
        assert_eq!((right.x, right.pointer), (1180.0, 90.0));
    }

    #[test]
    fn a_busy_mark_turns_once_a_second_unless_reduced_or_held() {
        assert!(near(spin(12.25, false, None), 0.25));
        assert_eq!(spin(12.25, true, None), 0.0);
        assert!(
            near(spin(12.25, true, Some(1250.0)), 0.25),
            "a check's held moment wins"
        );
        assert!(near(spin(0.0, false, Some(500.0)), 0.5));
        // It redraws with the clock only while it spins.
        assert!(spins(false, None));
        assert!(!spins(true, None) && !spins(false, Some(500.0)));
    }
}
