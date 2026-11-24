//! The file preview player's logic, separate from drawing and decoding: time formatting, where
//! a scrub bar press lands, what keys do, and which details a file lists.
//!
//! The controls match Chromium's `<video controls>` and `<audio controls>`, so the key rules are
//! Chromium's: Space or Return toggles play, Left and Right seek 1% of the duration (the
//! slider's default step), Home and End go to the ends, and Up and Down change the volume by 5%.

/// A time formatted as Chromium's media controls do: `M:SS`, or `H:MM:SS` from an hour on. NaN
/// gives `0:00`.
pub fn format_timestamp(seconds: f64) -> String {
    if !seconds.is_finite() {
        return "0:00".to_owned();
    }
    let whole = seconds.max(0.0).floor() as u64;
    let (hours, minutes, rest) = (whole / 3600, whole % 3600 / 60, whole % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}

/// The player's time text: position over duration.
pub fn clock_text(position_seconds: f64, duration_seconds: f64) -> String {
    let (position, length) = clock_parts(position_seconds, duration_seconds);
    format!("{position} {length}")
}

/// The time text in the two parts the video controls draw separately: "0:03" and "/ 0:05".
pub fn clock_parts(position_seconds: f64, duration_seconds: f64) -> (String, String) {
    (
        format_timestamp(position_seconds),
        format!("/ {}", format_timestamp(duration_seconds)),
    )
}

/// The filled fraction of the scrub bar; 0 when the duration is unknown.
pub fn played_share(position_seconds: f64, duration_seconds: f64) -> f64 {
    if duration_seconds.is_finite() && duration_seconds > 0.0 {
        (position_seconds / duration_seconds).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The pointer's fraction (0 to 1) along a scrub bar at `left`, `width` wide. `None` when the
/// bar has no width.
pub fn share_at_pointer(x: f64, left: f64, width: f64) -> Option<f64> {
    (width > 0.0).then(|| ((x - left) / width).clamp(0.0, 1.0))
}

/// The time at fraction `share` of `duration_seconds`. `None` when the duration is unknown.
pub fn seconds_at_share(share: f64, duration_seconds: f64) -> Option<f64> {
    (duration_seconds.is_finite() && duration_seconds > 0.0).then_some(share * duration_seconds)
}

/// The time under a pointer at `x` on a scrub bar at `left`, `width` wide. `None` when the
/// duration is unknown or the bar has no width.
pub fn seconds_at_pointer(x: f64, left: f64, width: f64, duration_seconds: f64) -> Option<f64> {
    seconds_at_share(share_at_pointer(x, left, width)?, duration_seconds)
}

/// A key the player handles while it has keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Space,
    Return,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Escape,
}

impl Key {
    /// Maps an FFI key to a player key, if the player handles it.
    pub fn from_press(key: crate::ffi::NrKey) -> Option<Self> {
        use crate::ffi::NrKey;
        Some(match key {
            NrKey::Space => Self::Space,
            NrKey::Return => Self::Return,
            NrKey::Left => Self::Left,
            NrKey::Right => Self::Right,
            NrKey::Up => Self::Up,
            NrKey::Down => Self::Down,
            NrKey::Home => Self::Home,
            NrKey::End => Self::End,
            NrKey::Escape => Self::Escape,
            _ => return None,
        })
    }
}

/// The action a key triggers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyAction {
    TogglePlaying,
    Seek(f64),
    Volume(f32),
}

/// The fraction of the duration one press of Left or Right seeks.
const SEEK_KEY_SHARE: f64 = 0.01;
/// The volume change per press of Up or Down.
const VOLUME_KEY_STEP: f32 = 0.05;

/// What `key` does at `position_seconds` of `duration_seconds` with the volume at `volume`.
/// `None` when it does nothing, such as a seek key before the duration is known.
pub fn key_action(
    key: Key,
    position_seconds: f64,
    duration_seconds: f64,
    volume: f32,
) -> Option<KeyAction> {
    let known = duration_seconds.is_finite() && duration_seconds > 0.0;
    let step = duration_seconds * SEEK_KEY_SHARE;
    let seek = |seconds: f64| known.then(|| KeyAction::Seek(seconds.clamp(0.0, duration_seconds)));
    match key {
        Key::Space | Key::Return => Some(KeyAction::TogglePlaying),
        Key::Left => seek(position_seconds - step),
        Key::Right => seek(position_seconds + step),
        Key::Home => seek(0.0),
        Key::End => seek(duration_seconds),
        // Snap to the step, so twenty presses span the range exactly.
        Key::Up => Some(KeyAction::Volume(snap_volume(volume + VOLUME_KEY_STEP))),
        Key::Down => Some(KeyAction::Volume(snap_volume(volume - VOLUME_KEY_STEP))),
        // Escape closes the menu or leaves full screen (`Preview::key`); it does not seek.
        Key::Escape => None,
    }
}

fn snap_volume(volume: f32) -> f32 {
    ((volume / VOLUME_KEY_STEP).round() * VOLUME_KEY_STEP).clamp(0.0, 1.0)
}

/// A playing file's clock: the instant and media time it was last anchored at, and its rate
/// ("Playback speed"). A rate change re-anchors at the current position, so time never jumps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub at: std::time::Instant,
    pub from: f64,
    pub rate: f64,
}

impl Anchor {
    /// The media time at `now`.
    pub fn position(&self, now: std::time::Instant) -> f64 {
        self.from + now.saturating_duration_since(self.at).as_secs_f64() * self.rate
    }

    /// The same clock re-anchored at `now` to run at `rate`.
    pub fn at_rate(&self, now: std::time::Instant, rate: f64) -> Self {
        Self {
            at: now,
            from: self.position(now),
            rate,
        }
    }

    /// The wall time from `now` until media time `due`; zero once it has passed.
    pub fn until(&self, due: f64, now: std::time::Instant) -> std::time::Duration {
        let seconds = (due - self.position(now)) / self.rate;
        if seconds.is_finite() && seconds > 0.0 {
            std::time::Duration::from_secs_f64(seconds)
        } else {
            std::time::Duration::ZERO
        }
    }
}

/// The requested rate, clamped to Chromium's range (0.25 to 2); NaN gives normal speed.
pub fn playback_rate(asked: f64) -> f64 {
    if asked.is_finite() {
        asked.clamp(
            video_controls::SPEEDS[0].0,
            video_controls::SPEEDS[video_controls::SPEEDS.len() - 1].0,
        )
    } else {
        1.0
    }
}

/// The details a file can list. `None` fields are omitted.
#[derive(Clone, Debug, Default)]
pub struct Details {
    pub kind: String,
    pub size: Option<String>,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub dimensions: Option<(u32, u32)>,
    pub duration_seconds: Option<f64>,
}

/// The line under a file's name in the details pane: its kind, then its size if known
/// ("ZIP archive · 327 KB").
pub fn summary_line(details: &Details) -> String {
    match details.size.as_deref().filter(|size| *size != "--") {
        Some(size) => format!("{} · {size}", details.kind),
        None => details.kind.clone(),
    }
}

/// The details under a file's picture, in order: Kind, Size, Date Created, Date Modified,
/// Dimensions, Duration, labelled in `language`.
pub fn details_lines(details: &Details, language: &str) -> Vec<(&'static str, String)> {
    use crate::strings::{key, text};
    let mut lines = vec![(
        text(language, key::PLAYER_DETAIL_KIND),
        details.kind.clone(),
    )];
    let mut add = |label: crate::strings::Key, value: Option<String>| {
        if let Some(value) = value.filter(|value| value != "--") {
            lines.push((text(language, label), value));
        }
    };
    add(key::PLAYER_DETAIL_SIZE, details.size.clone());
    add(key::PLAYER_DETAIL_CREATED, details.created.clone());
    add(key::PLAYER_DETAIL_MODIFIED, details.modified.clone());
    add(
        key::PLAYER_DETAIL_DIMENSIONS,
        details
            .dimensions
            .map(|(width, height)| format!("{width} × {height}")),
    );
    add(
        key::PLAYER_DETAIL_DURATION,
        details
            .duration_seconds
            .filter(|seconds| seconds.is_finite())
            .map(crate::list::format_duration),
    );
    lines
}

/// Parses a UTC time written as `YYYY-MM-DDTHH:MM:SS`, with optional fractional seconds and a
/// trailing `Z`. Anything else is `None`.
pub fn parse_utc(text: &str) -> Option<i64> {
    let text = text.strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.split('-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let mut time = time.split(':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    let in_range = (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..61).contains(&second);
    in_range.then(|| {
        crate::calendar::days_from_civil(year, month, day) * 86_400
            + hour * 3600
            + minute * 60
            + second
    })
}

/// The video controls, laid out like Chromium's `<video controls>`: the boxes, colours and
/// timings of `-webkit-media-controls-*`, measured in Chromium. Points from the picture's
/// top-left corner.
pub mod video_controls {
    use super::Key;
    use crate::glyph::MediaGlyph;
    use crate::motion::{Curve, Held};

    /// A box: left, top, width, height.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct Rect {
        pub x: f64,
        pub y: f64,
        pub width: f64,
        pub height: f64,
    }

    impl Rect {
        /// Whether (`x`, `y`) is inside, as AppKit tests it: left and top edges inclusive, right
        /// and bottom exclusive. An empty box contains nothing.
        pub fn contains(&self, x: f64, y: f64) -> bool {
            x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
        }

        /// The box inset by `dx` on the left and right and `dy` on the top and bottom (outset when
        /// negative).
        pub fn inset(&self, dx: f64, dy: f64) -> Rect {
            Rect {
                x: self.x + dx,
                y: self.y + dy,
                width: self.width - 2.0 * dx,
                height: self.height - 2.0 * dy,
            }
        }

        /// The bounding box of both.
        pub fn union(&self, other: Rect) -> Rect {
            let (left, top) = (self.x.min(other.x), self.y.min(other.y));
            let (right, bottom) = (
                (self.x + self.width).max(other.x + other.width),
                (self.y + self.height).max(other.y + other.height),
            );
            Rect {
                x: left,
                y: top,
                width: right - left,
                height: bottom - top,
            }
        }
    }

    /// A button's box size; its `GLYPH`-wide glyph is centred in it.
    pub const BUTTON: f64 = 48.0;
    pub const GLYPH: f64 = 20.0;
    /// The height of the round background behind a hovered button and the open volume control.
    pub const GROUND: f64 = 36.0;
    /// The scrub bar's row under the buttons.
    pub const TIMELINE: f64 = 24.0;
    /// How far above its row the scrub bar responds: hover shows the preview and knob, and a press
    /// scrubs. It reaches into the empty bottom of the button boxes, under their glyphs.
    pub const SCRUB_REACH: f64 = 12.0;
    /// Bar thickness and knob diameter, in points.
    pub const TRACK: f64 = 4.0;
    pub const KNOB: f64 = 12.0;
    /// The height of the gradient under the controls, from clear to black.
    pub const SHADE: f64 = 112.0;
    /// The time starts 48 pt in; "/ 0:04" follows "0:00" after this gap.
    pub const TIME_GAP: f64 = 4.0;
    /// The open volume control: the slider bar width and the background behind slider and speaker.
    pub const VOLUME_BAR: f64 = 52.0;
    /// White opacities over the picture: the bar background, the loaded part, the played part.
    pub const TRACK_ALPHA: f64 = 0.3;
    pub const LOADED_ALPHA: f64 = 0.54;
    /// The background of the open volume control and of a hovered button: rgb(32, 33, 36) at this
    /// opacity.
    pub const VOLUME_GROUND_ALPHA: f64 = 0.71;
    pub const GROUND_RGB: u32 = 0x20_21_24;

    /// The gradient stops: offset down its 112 pt, and black opacity.
    pub const SHADE_STOPS: [(f64, f64); 16] = [
        (0.0, 0.0),
        (0.081, 0.01),
        (0.155, 0.047),
        (0.225, 0.106),
        (0.29, 0.176),
        (0.353, 0.26),
        (0.412, 0.353),
        (0.471, 0.45),
        (0.529, 0.55),
        (0.588, 0.647),
        (0.647, 0.74),
        (0.71, 0.824),
        (0.775, 0.894),
        (0.845, 0.953),
        (0.919, 0.99),
        (1.0, 1.0),
    ];

    /// The layout over a `width` by `height` picture. A button without room (`buttons_that_fit`)
    /// has an empty box.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Layout {
        pub play: Rect,
        /// Where the time text starts, and the row it is centred in.
        pub time_x: f64,
        pub row: Rect,
        pub mute: Rect,
        pub fullscreen: Rect,
        pub overflow: Rect,
        /// The scrub bar itself, and the row that receives its presses.
        pub track: Rect,
        pub timeline: Rect,
        /// The open volume control: its background and slider bar. Both are empty when closed.
        pub volume_ground: Rect,
        pub volume_bar: Rect,
        pub shade: Rect,
    }

    /// The controls over a `width` by `height` picture, with the volume control open (pointer on
    /// the speaker or slider) or closed.
    pub fn layout(width: f64, height: f64, volume_open: bool) -> Layout {
        layout_at(width, height, if volume_open { 1.0 } else { 0.0 })
    }

    /// Which buttons fit in a `width`-wide picture.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct ButtonsThatFit {
        pub play: bool,
        pub mute: bool,
        pub fullscreen: bool,
        pub overflow: bool,
    }

    /// Chromium's rule for narrow pictures (`MediaControlsImpl::UpdateOverflowMenuWanted`, taken
    /// from its source, not measured): reserve the overflow button first, then add parts in order
    /// (play, speaker, full screen, time, duration) while each fits, and show nothing after the
    /// first that does not. Every button is `BUTTON` wide, so play needs 96 pt, the speaker 144 pt
    /// and full screen 192 pt; the time gets what the buttons leave (the shell measures it). A
    /// hidden part has an empty box, so nothing draws it and no press or hover reaches it.
    pub fn buttons_that_fit(width: f64) -> ButtonsThatFit {
        let overflow = width >= BUTTON;
        let mut room = if overflow { width - BUTTON } else { 0.0 };
        let mut still = true;
        let mut take = || {
            still = still && room >= BUTTON;
            if still {
                room -= BUTTON;
            }
            still
        };
        let play = take();
        let mute = take();
        let fullscreen = take();
        ButtonsThatFit {
            play,
            mute,
            fullscreen,
            overflow,
        }
    }

    /// The controls with the volume control `open` of the way open (0 closed, 1 open). It grows
    /// leftwards from the speaker: its background from 36 pt to 112 pt and its bar from 0 to
    /// `VOLUME_BAR`, both keeping their right edges.
    pub fn layout_at(width: f64, height: f64, open: f64) -> Layout {
        let top = height - TIMELINE - BUTTON;
        let button = |x: f64| Rect {
            x,
            y: top,
            width: BUTTON,
            height: BUTTON,
        };
        let fits = buttons_that_fit(width);
        // The buttons that fit stay right-aligned in order, with the overflow button last.
        let shown = |kept: bool, from_right: f64| {
            if kept {
                button(width - from_right * BUTTON)
            } else {
                Rect::default()
            }
        };
        let fullscreen = shown(fits.fullscreen, 2.0);
        let mute = shown(fits.mute, if fits.fullscreen { 3.0 } else { 2.0 });
        let middle = top + BUTTON / 2.0;
        let (volume_ground, volume_bar) = if open > 0.0 && fits.mute {
            // When open: 68 pt left of the speaker's box and 4 pt short of its right edge.
            let ground = GROUND + (VOLUME_GROUND - GROUND) * open;
            let bar = VOLUME_BAR * open;
            (
                Rect {
                    x: mute.x + BUTTON - 4.0 - ground,
                    y: top + (BUTTON - GROUND) / 2.0,
                    width: ground,
                    height: GROUND,
                },
                Rect {
                    x: mute.x - bar,
                    y: middle - TRACK / 2.0,
                    width: bar,
                    height: TRACK,
                },
            )
        } else {
            (Rect::default(), Rect::default())
        };
        Layout {
            play: if fits.play {
                button(0.0)
            } else {
                Rect::default()
            },
            time_x: BUTTON,
            row: Rect {
                x: 0.0,
                y: top,
                width,
                height: BUTTON,
            },
            mute,
            fullscreen,
            overflow: shown(fits.overflow, 1.0),
            track: Rect {
                x: 16.0,
                y: height - TIMELINE,
                width: (width - 32.0).max(0.0),
                height: TRACK,
            },
            timeline: Rect {
                x: 0.0,
                y: height - TIMELINE,
                width,
                height: TIMELINE,
            },
            volume_ground,
            volume_bar,
            shade: Rect {
                x: 0.0,
                y: (height - SHADE).max(0.0),
                width,
                height: SHADE.min(height),
            },
        }
    }

    /// Where the time's two parts start, given their measured widths (`first_width` "0:03",
    /// `second_width` "/ 0:04"). Each shows only when every button fits and the text ends before
    /// the speaker, the second a gap after the first. They rank after the buttons in Chromium's
    /// order (`buttons_that_fit`).
    pub fn time_places(
        laid: &Layout,
        first_width: f64,
        second_width: f64,
    ) -> (Option<f64>, Option<f64>) {
        let every_button = [laid.play, laid.mute, laid.fullscreen, laid.overflow]
            .iter()
            .all(|button| button.width > 0.0);
        let end = laid.mute.x;
        if !(every_button && laid.time_x + first_width <= end) {
            return (None, None);
        }
        let second = laid.time_x + first_width.ceil() + TIME_GAP;
        (
            Some(laid.time_x),
            (second + second_width <= end).then_some(second),
        )
    }

    /// How much of a `preload="metadata"` file Chromium reads: about 1 MiB beyond what it needs, in
    /// 64 KiB blocks, measured as 17 blocks for one file and 18 for another.
    pub const PRELOADED_BYTES: u64 = 17 * 65_536;

    /// The scrub bar's loaded fraction: what Chromium has read before first play, and all of it
    /// once played (a local file reads far faster than it plays).
    pub fn loaded_share(file_bytes: u64, played: bool) -> f64 {
        if played || file_bytes <= PRELOADED_BYTES {
            1.0
        } else {
            PRELOADED_BYTES as f64 / file_bytes as f64
        }
    }

    /// Where the pointer is.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Pointer {
        /// Not over the picture.
        Away,
        /// Over the picture, not over the controls.
        OverPicture,
        /// Over the buttons or scrub bar, holding one of them, or with a menu open.
        OnControls,
    }

    /// The controls stay 3 s after the pointer last moved over a playing picture, then fade.
    /// Chromium fades two nested layers (from its media controls stylesheet, measured with the
    /// controls held part-way): the panel with the gradient, out over 1 s and in over 250 ms; and
    /// the buttons and scrub bar inside it, out over 750 ms and in over 500 ms. All use CSS `ease`.
    /// A button's opacity is the product of both; the gradient uses the panel's alone.
    pub const IDLE_MS: f64 = 3000.0;
    pub const FADE_OUT_MS: f64 = 1000.0;
    pub const FADE_IN_MS: f64 = 250.0;
    pub const BUTTONS_FADE_OUT_MS: f64 = 750.0;
    pub const BUTTONS_FADE_IN_MS: f64 = 500.0;
    /// The open volume control's background at its widest.
    pub const VOLUME_GROUND: f64 = 112.0;
    /// The volume control's widths animate over 300 ms and its background fades over 250 ms, both
    /// on `ease`. When closing, the slider stays for 280 ms and then disappears at once
    /// (`steps(1)`).
    pub const VOLUME_MS: f64 = 300.0;
    pub const VOLUME_GROUND_FADE_MS: f64 = 250.0;
    pub const VOLUME_SLIDER_LINGER_MS: f64 = 280.0;
    /// A button's round background fades in and out over 250 ms on `ease`.
    pub const HOVER_GROUND_MS: f64 = 250.0;
    /// The menu and speed list scale from their bottom-right corner over 300 ms on `ease-out` and
    /// fade in over 200 ms linearly. Rows follow bottom first: the last row fades in over 400 ms on
    /// `ease-in` after 100 ms; each row above rises 15 pt into place over 200 ms and fades in over
    /// 400 ms, both on `ease`, 50 ms after the row below (150, 200, 250).
    pub const MENU_GROW_MS: f64 = 300.0;
    pub const MENU_SHOW_MS: f64 = 200.0;
    pub const MENU_ROW_SHOW_MS: f64 = 400.0;
    pub const MENU_ROW_RISE_MS: f64 = 200.0;
    pub const MENU_ROW_RISE: f64 = 15.0;
    pub const MENU_ROW_FIRST_DELAY_MS: f64 = 100.0;
    pub const MENU_ROW_DELAY_STEP_MS: f64 = 50.0;

    /// The two layers that fade.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Layer {
        /// The panel with the gradient.
        Panel,
        /// The buttons, the time and the scrub bar, inside the panel.
        Buttons,
    }

    /// Whether the controls should show: always over a paused picture or while the pointer is on
    /// them; over a playing one only until the pointer has rested `idle_ms` on the picture, and not
    /// once it has left.
    pub fn wanted(playing: bool, pointer: Pointer, idle_ms: f64) -> bool {
        !playing
            || pointer == Pointer::OnControls
            || (pointer == Pointer::OverPicture && idle_ms < IDLE_MS)
    }

    /// The opacity of `layer` `since_ms` after the controls began showing (`showing`) or hiding,
    /// starting from `from`.
    pub fn opacity(layer: Layer, showing: bool, from: f64, since_ms: f64) -> f64 {
        let (fade_in, fade_out) = match layer {
            Layer::Panel => (FADE_IN_MS, FADE_OUT_MS),
            Layer::Buttons => (BUTTONS_FADE_IN_MS, BUTTONS_FADE_OUT_MS),
        };
        towards(
            showing,
            0.0,
            1.0,
            from,
            since_ms,
            if showing { fade_in } else { fade_out },
            Curve::Ease,
        )
    }

    /// A value moving from `from` to `high` (`rising`) or `low` over `duration_ms` on `curve`,
    /// `since_ms` after it started, as a CSS transition continues from wherever it was.
    fn towards(
        rising: bool,
        low: f64,
        high: f64,
        from: f64,
        since_ms: f64,
        duration_ms: f64,
        curve: Curve,
    ) -> f64 {
        let to = if rising { high } else { low };
        from + (to - from) * curve.at(since_ms / duration_ms)
    }

    /// The volume control `since_ms` after it began opening (`open`) or closing from `from_share`
    /// open with background opacity `from_strength`. Returns how far open it is (for
    /// `layout_at`), its background opacity, and whether the slider shows.
    pub fn volume_move(
        open: bool,
        from_share: f64,
        from_strength: f64,
        since_ms: f64,
    ) -> (f64, f64, bool) {
        let share = towards(open, 0.0, 1.0, from_share, since_ms, VOLUME_MS, Curve::Ease);
        let strength = towards(
            open,
            0.0,
            VOLUME_GROUND_ALPHA,
            from_strength,
            since_ms,
            VOLUME_GROUND_FADE_MS,
            Curve::Ease,
        );
        let slider = open || since_ms < VOLUME_SLIDER_LINGER_MS;
        (share, strength, slider && share > 0.0)
    }

    /// A button's round background opacity `since_ms` after the pointer entered (`on`) or left,
    /// starting from `from`, up to `VOLUME_GROUND_ALPHA`.
    pub fn hover_strength(on: bool, from: f64, since_ms: f64) -> f64 {
        towards(
            on,
            0.0,
            VOLUME_GROUND_ALPHA,
            from,
            since_ms,
            HOVER_GROUND_MS,
            Curve::Ease,
        )
    }

    /// The menu or speed list `since_ms` after opening: its scale (about its bottom-right corner)
    /// and opacity.
    pub fn menu_entrance(since_ms: f64) -> (f64, f64) {
        (
            Curve::EaseOut.at(since_ms / MENU_GROW_MS),
            Curve::Linear.at(since_ms / MENU_SHOW_MS),
        )
    }

    /// A menu row `since_ms` after opening, `from_bottom` rows above the last: its opacity and how
    /// far below its final place it still is.
    pub fn menu_row_entrance(from_bottom: usize, since_ms: f64) -> (f64, f64) {
        let delay = MENU_ROW_FIRST_DELAY_MS + MENU_ROW_DELAY_STEP_MS * from_bottom as f64;
        let time = since_ms - delay;
        if from_bottom == 0 {
            return (Curve::EaseIn.at(time / MENU_ROW_SHOW_MS), 0.0);
        }
        (
            Curve::Ease.at(time / MENU_ROW_SHOW_MS),
            MENU_ROW_RISE * (1.0 - Curve::Ease.at(time / MENU_ROW_RISE_MS)),
        )
    }

    /// The volume for a press at `x` on the open slider bar.
    pub fn volume_at(x: f64, bar: Rect) -> f32 {
        if bar.width <= 0.0 {
            return 1.0;
        }
        ((x - bar.x) / bar.width).clamp(0.0, 1.0) as f32
    }

    /// The speeds in Chromium's "Playback speed" list and their labels: the number, or `None` for
    /// normal speed, which gets a word label.
    pub const SPEEDS: [(f64, Option<&str>); 8] = [
        (0.25, Some("0.25")),
        (0.5, Some("0.5")),
        (0.75, Some("0.75")),
        (1.0, None),
        (1.25, Some("1.25")),
        (1.5, Some("1.5")),
        (1.75, Some("1.75")),
        (2.0, Some("2")),
    ];

    /// A button of the controls.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Button {
        Play,
        Mute,
        Fullscreen,
        Overflow,
    }

    /// The button under a pointer at (`x`, `y`); its whole 48 pt box responds, as in Chromium. A
    /// file without sound has no speaker.
    pub fn button_at(laid: &Layout, x: f64, y: f64, has_audio: bool) -> Option<Button> {
        let over = |rect: Rect| {
            x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
        };
        if over(laid.play) {
            Some(Button::Play)
        } else if has_audio && over(laid.mute) {
            Some(Button::Mute)
        } else if over(laid.fullscreen) {
            Some(Button::Fullscreen)
        } else if over(laid.overflow) {
            Some(Button::Overflow)
        } else {
            None
        }
    }

    /// The hovered button's round background: rgb(32, 33, 36) at `VOLUME_GROUND_ALPHA`, `GROUND`
    /// across, centred on the button (measured in Chromium with the pointer on play and on the
    /// overflow button). The speaker opens its volume control instead, whose background is the same
    /// colour, and an open menu captures the pointer.
    pub fn hover_ground(
        laid: &Layout,
        x: f64,
        y: f64,
        has_audio: bool,
        menu: &Menu,
    ) -> Option<Rect> {
        if *menu != Menu::Closed {
            return None;
        }
        let button = match button_at(laid, x, y, has_audio)? {
            Button::Play => laid.play,
            Button::Fullscreen => laid.fullscreen,
            Button::Overflow => laid.overflow,
            Button::Mute => return None,
        };
        Some(Rect {
            x: button.x + (button.width - GROUND) / 2.0,
            y: button.y + (button.height - GROUND) / 2.0,
            width: GROUND,
            height: GROUND,
        })
    }

    /// The target of a press on a player's controls.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Press {
        /// A resting page player opens its file first, then handles the press.
        Wake,
        /// The open menu captures every press (`Menu::pressed`).
        Menu,
        /// The overflow button: opens the menu, or closes whatever is open.
        ToggleMenu,
        Fullscreen,
        Play,
        /// The open volume slider: the press sets the volume, and dragging along `bar` adjusts it.
        Volume(Rect),
        Mute,
        /// The scrub bar: seeks to the time under the press on `track`, and dragging scrubs,
        /// pausing playback meanwhile.
        Scrub(Rect),
        /// The picture: toggles play; the second press of a double click also toggles full screen,
        /// as in Chromium.
        Picture,
    }

    /// The target of a press at (`x`, `y`) over a `width` by `height` picture, checked in order:
    /// the open menu, the scrub bar's hit area (`scrubs_at`), the overflow button, full screen,
    /// play, the open volume slider (off the speaker), the speaker, else the picture. A file
    /// without sound has neither slider nor speaker.
    pub fn press_at(
        width: f64,
        height: f64,
        x: f64,
        y: f64,
        has_audio: bool,
        volume_open: bool,
        menu_open: bool,
    ) -> Press {
        let laid = layout(width, height, true);
        if menu_open {
            return Press::Menu;
        }
        if scrubs_at(&laid, x, y, has_audio && volume_open) {
            return Press::Scrub(laid.track);
        }
        button_press(&laid, x, y)
            .or_else(|| {
                has_audio
                    .then(|| sound_press(&laid, x, y, volume_open))
                    .flatten()
            })
            .unwrap_or(Press::Picture)
    }

    /// The overflow, full screen or play button under (`x`, `y`), checked in that order.
    fn button_press(laid: &Layout, x: f64, y: f64) -> Option<Press> {
        if laid.overflow.contains(x, y) {
            Some(Press::ToggleMenu)
        } else if laid.fullscreen.contains(x, y) {
            Some(Press::Fullscreen)
        } else if laid.play.contains(x, y) {
            Some(Press::Play)
        } else {
            None
        }
    }

    /// The sound control under (`x`, `y`): the open volume slider off the speaker, else the
    /// speaker.
    fn sound_press(laid: &Layout, x: f64, y: f64, volume_open: bool) -> Option<Press> {
        let on_mute = laid.mute.contains(x, y);
        if volume_open && laid.volume_ground.contains(x, y) && !on_mute {
            Some(Press::Volume(laid.volume_bar))
        } else if on_mute {
            Some(Press::Mute)
        } else {
            None
        }
    }

    /// The scrub bar's hit area: its row plus `SCRUB_REACH` above it.
    pub fn scrub_band(laid: &Layout) -> Rect {
        laid.timeline.inset(0.0, -SCRUB_REACH)
    }

    /// Whether the scrub bar responds to the pointer at (`x`, `y`): inside `scrub_band`, but not on
    /// the open volume slider, whose bottom overlaps the band's top.
    pub fn scrubs_at(laid: &Layout, x: f64, y: f64, volume_open: bool) -> bool {
        scrub_band(laid).contains(x, y) && !(volume_open && laid.volume_ground.contains(x, y))
    }

    /// Whether a press at (`x`, `y`) over a `width` by `height` picture belongs to the controls,
    /// not the picture: on the button row, the scrub bar's hit area (`scrub_band`), the open volume
    /// control, or anywhere while the menu is open.
    pub fn operates(
        width: f64,
        height: f64,
        x: f64,
        y: f64,
        volume_open: bool,
        menu_open: bool,
    ) -> bool {
        if !(Rect {
            x: 0.0,
            y: 0.0,
            width,
            height,
        })
        .contains(x, y)
        {
            return false;
        }
        let laid = layout(width, height, true);
        menu_open
            || laid.row.union(scrub_band(&laid)).contains(x, y)
            || (volume_open && laid.volume_ground.contains(x, y))
    }

    /// Chromium's overflow menu, as measured: a white 200 pt sheet with 2 px corners and the shadow
    /// `0 1px 9px rgba(0, 0, 0, 0.4)`, its right edge 4 pt in from the picture's and its bottom
    /// 28 pt above the picture's, at most 250 pt tall.
    pub const MENU_WIDTH: f64 = 200.0;
    pub const MENU_RIGHT: f64 = 4.0;
    pub const MENU_BOTTOM: f64 = 28.0;
    pub const MENU_TALLEST: f64 = 250.0;
    pub const MENU_CORNER: f64 = 2.0;
    pub const MENU_SHADOW: (f64, f64, f64) = (1.0, 9.0, 0.4);
    /// The focused row is #e0e0e0; text is black at 0.87 and the second line ("Off") at 0.54, in
    /// 12 px on 14 px lines; the tick is #4285f4.
    pub const MENU_FOCUS_GREY: f64 = 224.0;
    pub const MENU_TICK_RGB: u32 = 0x42_85_F4;
    pub const MENU_INK_ALPHA: f64 = 0.87;
    pub const MENU_QUIET_ALPHA: f64 = 0.54;
    pub const MENU_FONT: f64 = 12.0;
    pub const MENU_LINE: f64 = 14.0;
    /// A row's 24 px icon starts 16 pt in and its label 56 pt in; in a choice list the label starts
    /// 58 pt in and the 18 px tick ends 31 pt short of the right edge.
    pub const MENU_ICON_X: f64 = 16.0;
    pub const MENU_ICON: f64 = 24.0;
    pub const MENU_WORDS_X: f64 = 56.0;
    pub const MENU_CHOICE_X: f64 = 58.0;
    pub const MENU_TICK: f64 = 18.0;
    pub const MENU_TICK_RIGHT: f64 = 31.0;
    /// The height of a choice list row and its heading.
    pub const MENU_CHOICE_ROW: f64 = 48.0;

    /// A row of the menu.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum MainRow {
        Download,
        Captions,
        PlaybackSpeed,
        PictureInPicture,
    }

    impl MainRow {
        /// The icon at the row's left.
        pub fn glyph(self) -> MediaGlyph {
            match self {
                MainRow::Download => MediaGlyph::Download,
                MainRow::Captions => MediaGlyph::Captions,
                MainRow::PlaybackSpeed => MediaGlyph::Speed,
                MainRow::PictureInPicture => MediaGlyph::PictureInPicture,
            }
        }
    }

    /// The menu rows in order, each with its label and height (44 pt for the two-line row, 46 pt
    /// for the others).
    pub const MAIN_ROWS: [(MainRow, crate::strings::Key, f64); 4] = [
        (
            MainRow::Download,
            crate::strings::key::PLAYER_MENU_DOWNLOAD,
            46.0,
        ),
        (
            MainRow::Captions,
            crate::strings::key::PLAYER_MENU_CAPTIONS,
            44.0,
        ),
        (
            MainRow::PlaybackSpeed,
            crate::strings::key::PLAYER_MENU_SPEED,
            46.0,
        ),
        (
            MainRow::PictureInPicture,
            crate::strings::key::PLAYER_MENU_PICTURE_IN_PICTURE,
            46.0,
        ),
    ];

    /// The overflow menu.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub enum Menu {
        #[default]
        Closed,
        /// The four rows; the `focus` row has keyboard focus and the grey background.
        Main { focus: usize },
        /// The speed list: row 0 is its heading, rows 1 to 8 the speeds. It is taller than the
        /// sheet, so it scrolls by `scroll`.
        Speeds { focus: usize, scroll: f64 },
    }

    /// What a press or key on the menu triggers beyond changing the menu itself.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Chosen {
        Nothing,
        /// Save a copy of the file.
        Download,
        /// Captions and Picture in Picture are drawn but do nothing here.
        Captions,
        PictureInPicture,
        /// Play at this speed.
        Rate(f64),
    }

    impl Menu {
        pub fn is_open(&self) -> bool {
            *self != Self::Closed
        }

        /// The overflow button: opens the menu with its first row focused, or closes whatever is
        /// open.
        pub fn toggled(self) -> Self {
            match self {
                Self::Closed => Self::Main { focus: 0 },
                _ => Self::Closed,
            }
        }

        /// Row `index`'s label in `language`; with `second`, its second line ("Off" under Captions,
        /// for a file without captions). `None` for a row without one. The speed list's heading is
        /// the Playback speed label and leads back to the menu.
        pub fn label(&self, index: usize, second: bool, language: &str) -> Option<&'static str> {
            use crate::strings::{key, text};
            match self {
                Self::Main { .. } => {
                    MAIN_ROWS
                        .get(index)
                        .and_then(|(row, name, _)| match (second, row) {
                            (false, _) => Some(text(language, *name)),
                            (true, MainRow::Captions) => {
                                Some(text(language, key::PLAYER_CAPTIONS_OFF))
                            }
                            (true, _) => None,
                        })
                }
                Self::Speeds { .. } if !second && index == 0 => {
                    Some(text(language, key::PLAYER_MENU_SPEED))
                }
                Self::Speeds { .. } if !second => SPEEDS.get(index - 1).map(|(_, number)| {
                    number.unwrap_or_else(|| text(language, key::PLAYER_SPEED_NORMAL))
                }),
                _ => None,
            }
        }

        pub fn rows(&self) -> usize {
            match self {
                Self::Closed => 0,
                Self::Main { .. } => MAIN_ROWS.len(),
                Self::Speeds { .. } => SPEEDS.len() + 1,
            }
        }

        pub fn focus(&self) -> Option<usize> {
            match self {
                Self::Closed => None,
                Self::Main { focus } | Self::Speeds { focus, .. } => Some(*focus),
            }
        }

        /// The FFI number for what is open: 0 nothing, 1 the menu, 2 the speed list.
        pub fn kind(&self) -> crate::ffi::NrMenuShown {
            use crate::ffi::NrMenuShown;
            match self {
                Self::Closed => NrMenuShown::Closed,
                Self::Main { .. } => NrMenuShown::Main,
                Self::Speeds { .. } => NrMenuShown::Speeds,
            }
        }

        /// The icon at row `index`'s left: each menu row's own, and a back icon on the speed list's
        /// heading.
        pub fn row_glyph(&self, index: usize) -> Option<MediaGlyph> {
            match self {
                Self::Closed => None,
                Self::Main { .. } => MAIN_ROWS.get(index).map(|row| row.0.glyph()),
                Self::Speeds { .. } => (index == 0).then_some(MediaGlyph::Back),
            }
        }

        /// Row `index`'s offset from the top of all rows, and its height.
        fn row_span(&self, index: usize) -> Option<(f64, f64)> {
            match self {
                Self::Closed => None,
                Self::Main { .. } => {
                    let (_, _, tall) = MAIN_ROWS.get(index)?;
                    Some((MAIN_ROWS[..index].iter().map(|row| row.2).sum(), *tall))
                }
                Self::Speeds { .. } => (index <= SPEEDS.len())
                    .then_some((index as f64 * MENU_CHOICE_ROW, MENU_CHOICE_ROW)),
            }
        }

        fn content_height(&self) -> f64 {
            self.row_span(self.rows().saturating_sub(1))
                .map_or(0.0, |(top, tall)| top + tall)
        }

        /// The sheet's frame over a `width` by `height` picture; empty when closed.
        pub fn frame(&self, width: f64, height: f64) -> Rect {
            if !self.is_open() {
                return Rect::default();
            }
            let tall = self
                .content_height()
                .min(MENU_TALLEST)
                .min((height - MENU_BOTTOM - MENU_RIGHT).max(0.0));
            Rect {
                x: width - MENU_RIGHT - MENU_WIDTH,
                y: height - MENU_BOTTOM - tall,
                width: MENU_WIDTH,
                height: tall,
            }
        }

        /// The speed list's scroll offset; 0 otherwise.
        pub fn scroll(&self) -> f64 {
            match self {
                Self::Speeds { scroll, .. } => *scroll,
                _ => 0.0,
            }
        }

        fn furthest_scroll(&self, width: f64, height: f64) -> f64 {
            (self.content_height() - self.frame(width, height).height).max(0.0)
        }

        /// Row `index`'s frame over the picture, after scrolling. It may lie partly or wholly
        /// outside the sheet, which clips it.
        pub fn row(&self, index: usize, width: f64, height: f64) -> Option<Rect> {
            let (top, tall) = self.row_span(index)?;
            let sheet = self.frame(width, height);
            Some(Rect {
                x: sheet.x,
                y: sheet.y + top - self.scroll(),
                width: sheet.width,
                height: tall,
            })
        }

        /// The row under (`x`, `y`), when the point is on the sheet.
        pub fn row_at(&self, x: f64, y: f64, width: f64, height: f64) -> Option<usize> {
            let sheet = self.frame(width, height);
            if !(x >= sheet.x
                && x < sheet.x + sheet.width
                && y >= sheet.y
                && y < sheet.y + sheet.height)
            {
                return None;
            }
            (0..self.rows()).find(|index| {
                self.row(*index, width, height)
                    .is_some_and(|row| y >= row.y && y < row.y + row.height)
            })
        }

        /// The scroll that centres row `focus` in the sheet, as Chromium reveals the ticked speed,
        /// clamped to the scroll range.
        fn centred(focus: usize, width: f64, height: f64) -> Self {
            let list = Self::Speeds { focus, scroll: 0.0 };
            let (top, tall) = list.row_span(focus).unwrap_or((0.0, MENU_CHOICE_ROW));
            let scroll = (top + tall / 2.0 - list.frame(width, height).height / 2.0)
                .clamp(0.0, list.furthest_scroll(width, height));
            Self::Speeds { focus, scroll }
        }

        /// The speed list as it opens while playing at `rate`, with the ticked speed focused and
        /// centred.
        pub fn speeds(rate: f64, width: f64, height: f64) -> Self {
            Self::centred(ticked_speed(rate) + 1, width, height)
        }

        /// Activates the focused row (press or Return): Download requests a copy and closes;
        /// Captions has nothing to list and stays; Playback speed opens the list; Picture in
        /// Picture closes; the list's heading goes back; a speed is chosen.
        pub fn activated(self, rate: f64, width: f64, height: f64) -> (Self, Chosen) {
            match self {
                Self::Closed => (self, Chosen::Nothing),
                Self::Main { focus } => match MAIN_ROWS.get(focus).map(|row| row.0) {
                    Some(MainRow::Download) => (Self::Closed, Chosen::Download),
                    Some(MainRow::Captions) => (self, Chosen::Captions),
                    Some(MainRow::PlaybackSpeed) => {
                        (Self::speeds(rate, width, height), Chosen::Nothing)
                    }
                    Some(MainRow::PictureInPicture) => (Self::Closed, Chosen::PictureInPicture),
                    None => (self, Chosen::Nothing),
                },
                Self::Speeds { focus: 0, .. } => (Self::Main { focus: 2 }, Chosen::Nothing),
                Self::Speeds { focus, .. } => match SPEEDS.get(focus - 1) {
                    Some((speed, _)) => (Self::Closed, Chosen::Rate(*speed)),
                    None => (self, Chosen::Nothing),
                },
            }
        }

        /// A press at (`x`, `y`): on a row it focuses and activates the row; off the sheet it only
        /// closes the menu.
        pub fn pressed(self, x: f64, y: f64, rate: f64, width: f64, height: f64) -> (Self, Chosen) {
            match (self.row_at(x, y, width, height), self) {
                (Some(row), Self::Main { .. }) => {
                    Self::Main { focus: row }.activated(rate, width, height)
                }
                (Some(row), Self::Speeds { scroll, .. }) => {
                    Self::Speeds { focus: row, scroll }.activated(rate, width, height)
                }
                _ => (Self::Closed, Chosen::Nothing),
            }
        }

        /// Up and Down move focus one row, stopping at the ends, and the list scrolls to follow;
        /// Return and Space activate the row; Escape closes. `None` for keys the menu ignores.
        pub fn key(
            self,
            key: super::Key,
            rate: f64,
            width: f64,
            height: f64,
        ) -> Option<(Self, Chosen)> {
            use super::Key;
            let focus = self.focus()?;
            let moved = |to: usize| match self {
                Self::Speeds { scroll, .. } => {
                    let list = Self::Speeds { focus: to, scroll };
                    let (top, tall) = list.row_span(to).unwrap_or((0.0, MENU_CHOICE_ROW));
                    let shown = list.frame(width, height).height;
                    // Scroll only as far as needed to bring the row fully into the sheet.
                    Self::Speeds {
                        focus: to,
                        scroll: scroll.max(top + tall - shown).min(top).max(0.0),
                    }
                }
                _ => Self::Main { focus: to },
            };
            Some(match key {
                Key::Up => (moved(focus.saturating_sub(1)), Chosen::Nothing),
                Key::Down => (moved((focus + 1).min(self.rows() - 1)), Chosen::Nothing),
                Key::Return | Key::Space => self.activated(rate, width, height),
                Key::Escape => (Self::Closed, Chosen::Nothing),
                _ => return None,
            })
        }

        /// Scrolls the speed list by `down` points of wheel travel.
        pub fn scrolled(self, down: f64, width: f64, height: f64) -> Self {
            match self {
                Self::Speeds { focus, scroll } => Self::Speeds {
                    focus,
                    scroll: (scroll + down).clamp(0.0, self.furthest_scroll(width, height)),
                },
                other => other,
            }
        }
    }

    /// The index in `SPEEDS` ticked while playing at `rate`: the nearest one.
    pub fn ticked_speed(rate: f64) -> usize {
        let apart = |index: usize| (SPEEDS[index].0 - rate).abs();
        (0..SPEEDS.len())
            .min_by(|a, b| apart(*a).total_cmp(&apart(*b)))
            .unwrap_or(3)
    }

    /// A full screen request.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum FullscreenAsk {
        /// The full screen button, or a double click on the picture.
        Toggle,
        /// Escape, or the system's own exit.
        Leave,
    }

    /// Whether the picture is full screen after `ask`.
    pub fn fullscreen_after(filling: bool, ask: FullscreenAsk) -> bool {
        match ask {
            FullscreenAsk::Toggle => !filling,
            FullscreenAsk::Leave => false,
        }
    }

    /// Where a `width` by `height` picture sits on a screen: fitted whole and centred, as Chromium
    /// shows a full screen video.
    pub fn fullscreen_fit(width: f64, height: f64, screen_width: f64, screen_height: f64) -> Rect {
        if !(width > 0.0 && height > 0.0 && screen_width > 0.0 && screen_height > 0.0) {
            return Rect {
                x: 0.0,
                y: 0.0,
                width: screen_width.max(0.0),
                height: screen_height.max(0.0),
            };
        }
        // The limiting side matches the screen exactly, without rounding.
        let (fitted_width, fitted_height) = if screen_width / width <= screen_height / height {
            (screen_width, height * (screen_width / width))
        } else {
            (width * (screen_height / height), screen_height)
        };
        Rect {
            x: (screen_width - fitted_width) / 2.0,
            y: (screen_height - fitted_height) / 2.0,
            width: fitted_width,
            height: fitted_height,
        }
    }

    /// The scrub bar preview: while the pointer is on or dragging the bar, a thumbnail of the time
    /// under it, drawn over the buttons and ending `PREVIEW_GAP` above the bar. The thumbnail fits
    /// in `PREVIEW_WIDE` by `PREVIEW_TALL` with the video's aspect ratio, stays `PREVIEW_EDGE`
    /// from the video's sides and top, has `PREVIEW_CORNER` corners, and shows rgb(32, 33, 36) at
    /// `VOLUME_GROUND_ALPHA` until its frame loads. The time sits in a pill of that colour at the
    /// thumbnail's bottom: a `PREVIEW_LINE`-tall line, `PREVIEW_INSET` above the bottom, with
    /// `PREVIEW_PAD` either side of the text.
    pub const PREVIEW_WIDE: f64 = 160.0;
    pub const PREVIEW_TALL: f64 = 120.0;
    pub const PREVIEW_EDGE: f64 = 8.0;
    pub const PREVIEW_CORNER: f64 = 4.0;
    pub const PREVIEW_LINE: f64 = 18.0;
    pub const PREVIEW_GAP: f64 = 8.0;
    pub const PREVIEW_INSET: f64 = 4.0;
    pub const PREVIEW_PAD: f64 = 6.0;
    /// A video too short for a thumbnail this tall gets no preview.
    pub const PREVIEW_LEAST: f64 = 32.0;
    /// The number of evenly spaced times the preview can show, so frames decoded on one pass of
    /// the pointer serve the next.
    pub const PREVIEW_STEPS: f64 = 100.0;
    /// How many steps either side of the pointer are decoded ahead, so the next one is ready when
    /// the pointer reaches it.
    pub const PREVIEW_AHEAD: f64 = 6.0;

    /// The scrub bar preview, in control points.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct ScrubPreview {
        pub picture: Rect,
        /// The time's line, centred at the thumbnail's bottom.
        pub line: Rect,
        /// The time under the pointer, which a press there seeks to.
        pub at_seconds: f64,
        /// The thumbnail's time: the start of the step under the pointer.
        pub moment_seconds: f64,
        /// The step size: the duration over `PREVIEW_STEPS`.
        pub step_seconds: f64,
    }

    /// The preview with the pointer at `x`, for controls laid out as `laid` over a `width`-wide
    /// video of `duration_seconds` with native size `shape`. `None` for a video without a duration
    /// or without room for a preview.
    pub fn scrub_preview(
        laid: &Layout,
        x: f64,
        width: f64,
        duration_seconds: f64,
        shape: (f64, f64),
    ) -> Option<ScrubPreview> {
        if !(duration_seconds.is_finite()
            && duration_seconds > 0.0
            && shape.0 > 0.0
            && shape.1 > 0.0)
        {
            return None;
        }
        let share = super::share_at_pointer(x, laid.track.x, laid.track.width)?;
        let step = (share * PREVIEW_STEPS).floor().min(PREVIEW_STEPS - 1.0);
        let bottom = laid.track.y - PREVIEW_GAP;
        let room_wide = PREVIEW_WIDE.min(width - 2.0 * PREVIEW_EDGE);
        let room_tall = PREVIEW_TALL.min(bottom - PREVIEW_EDGE);
        let scale = (room_wide / shape.0).min(room_tall / shape.1);
        // Whole points, never larger than the available space.
        let (wide, tall) = ((shape.0 * scale).floor(), (shape.1 * scale).floor());
        if tall < PREVIEW_LEAST {
            return None;
        }
        let left = (x - wide / 2.0)
            .round()
            .clamp(PREVIEW_EDGE, width - PREVIEW_EDGE - wide);
        Some(ScrubPreview {
            picture: Rect {
                x: left,
                y: bottom - tall,
                width: wide,
                height: tall,
            },
            line: Rect {
                x: left,
                y: bottom - PREVIEW_INSET - PREVIEW_LINE,
                width: wide,
                height: PREVIEW_LINE,
            },
            at_seconds: share * duration_seconds,
            moment_seconds: step / PREVIEW_STEPS * duration_seconds,
            step_seconds: duration_seconds / PREVIEW_STEPS,
        })
    }

    /// The control style: Chromium's `<video controls>` over a picture, or its `<audio controls>`
    /// pill.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Style {
        Video,
        Sound,
    }

    /// The state the controls display: the open file's, or a resting page player's, shown paused
    /// where it was left with no menu.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Playback {
        pub playing: bool,
        pub position_seconds: f64,
        pub duration_seconds: f64,
        pub has_audio: bool,
        pub muted: bool,
        pub volume: f32,
        pub fullscreen: bool,
        /// It has played since the file was opened, so the bar shows fully loaded.
        pub has_played: bool,
        pub menu: Menu,
        /// A page player whose file is not open; a press or key wakes it first.
        pub resting: bool,
    }

    impl Playback {
        /// Pause while it plays, else play.
        fn play_glyph(&self) -> MediaGlyph {
            if self.playing {
                MediaGlyph::Pause
            } else {
                MediaGlyph::Play
            }
        }

        /// The speaker, crossed out while muted or at zero volume.
        fn mute_glyph(&self) -> MediaGlyph {
            if self.muted || self.volume == 0.0 {
                MediaGlyph::Muted
            } else {
                MediaGlyph::Sound
            }
        }

        fn fullscreen_glyph(&self) -> MediaGlyph {
            if self.fullscreen {
                MediaGlyph::LeaveFullscreen
            } else {
                MediaGlyph::Fullscreen
            }
        }
    }

    /// The platform clock now (seconds), and the times at which a check freezes entrances and exits
    /// (`motion::Held`).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Clock {
        pub now: f64,
        pub entrance: Held,
        pub exit: Held,
    }

    impl Clock {
        /// Milliseconds since `start`, for an entrance (`entering`) or exit.
        fn since(&self, start: f64, entering: bool) -> f64 {
            crate::motion::elapsed(
                start,
                self.now,
                if entering { self.entrance } else { self.exit },
            )
        }
    }

    /// What a press holds until release: the volume slider bar or the scrub bar.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Hold {
        Volume(Rect),
        Scrub(Rect),
    }

    /// The result of dragging what a press holds.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Drag {
        Volume(f32),
        /// A fraction of the duration.
        Seek(f64),
    }

    /// Where a key goes: a resting page player wakes first (except for Escape), the open menu takes
    /// its keys and blocks all others, and the player handles the rest.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum KeyRoute {
        Wake,
        Menu(Key),
        Player(Key),
        Pass,
    }

    /// A round hover background fading in or out: its frame, starting opacity and start time.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Ground {
        rect: Rect,
        from: f64,
        since: f64,
    }

    /// How long ago, in seconds, a new player's initial animations are treated as having ended, so
    /// they start settled.
    const SETTLED_SECONDS: f64 = 10.0;
    /// Layers and backgrounds below this opacity are not drawn.
    const UNSEEN: f64 = 0.001;

    /// One player's changing control state: pointer position and last move, the volume control's
    /// animation, the two fading layers, the hover backgrounds, the menu entrance, and what a press
    /// holds. Times are seconds on the platform clock.
    #[derive(Clone, Debug, PartialEq)]
    pub struct Controls {
        style: Style,
        inside: bool,
        pointer: Option<(f64, f64)>,
        last_moved: f64,
        volume_open: bool,
        volume_from: (f64, f64),
        volume_since: f64,
        showing: bool,
        fade_from: (f64, f64),
        fade_start: f64,
        hover: Option<Ground>,
        left: Option<Ground>,
        menu_shown: (crate::ffi::NrMenuShown, f64),
        hold: Option<Hold>,
        /// The video's native size, for the scrub bar preview; `None` for audio or an unknown size.
        shape: Option<(f64, f64)>,
    }

    /// The video controls as currently drawn. A frame of `None` means the panel has faded out.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct VideoFrame {
        /// The opacities of the panel with its gradient, and of the buttons inside it.
        pub panel: f64,
        pub buttons: f64,
        /// The layout, with the volume control at its current openness.
        pub laid: Layout,
        /// Each button's glyph; `None` for a button without room, and for the speaker of a silent
        /// file.
        pub play: Option<MediaGlyph>,
        pub mute: Option<MediaGlyph>,
        pub fullscreen: Option<MediaGlyph>,
        pub overflow: Option<MediaGlyph>,
        /// The hover backgrounds with their opacities: the one the pointer left, fading out, then
        /// the one under it. One too faint to see is `None`.
        pub grounds: [Option<(Rect, f64)>; 2],
        /// Where the time's two parts start, when shown (`time_places`).
        pub time: (Option<f64>, Option<f64>),
        /// The open volume control: its background opacity, and the slider level when shown.
        pub volume: Option<(f64, Option<f64>)>,
        /// The scrub bar: the loaded and played fractions, and whether its knob shows.
        pub loaded: f64,
        pub played: f64,
        pub knob: bool,
        /// The open menu (`Menu::kind`), and the time since it opened.
        pub menu: crate::ffi::NrMenuShown,
        pub menu_since_ms: f64,
        /// The scrub bar preview, while the pointer is on or dragging the bar (`scrub_preview`).
        pub preview: Option<ScrubPreview>,
    }

    /// The audio pill as currently drawn: each button's glyph over the whole button.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct SoundFrame {
        pub laid: super::sound_controls::Layout,
        pub play: MediaGlyph,
        pub played: f64,
        pub knob: bool,
        /// The speaker, crossed out when muted or at zero volume; `None` without sound.
        pub mute: Option<MediaGlyph>,
        pub more: MediaGlyph,
    }

    impl Controls {
        pub fn new(style: Style, now: f64) -> Self {
            Self {
                style,
                inside: false,
                pointer: None,
                last_moved: now,
                volume_open: false,
                volume_from: (0.0, 0.0),
                volume_since: now - SETTLED_SECONDS,
                showing: true,
                fade_from: (1.0, 1.0),
                fade_start: now - SETTLED_SECONDS,
                hover: None,
                left: None,
                menu_shown: (crate::ffi::NrMenuShown::Closed, 0.0),
                hold: None,
                shape: None,
            }
        }

        pub fn style(&self) -> Style {
            self.style
        }

        /// Sets the video's native size, which the scrub bar preview uses for its aspect ratio.
        pub fn set_shape(&mut self, width: f64, height: f64) {
            self.shape = (width > 0.0 && height > 0.0).then_some((width, height));
        }

        /// The volume control now: how far open, its background opacity, and whether the slider
        /// shows.
        fn volume_now(&self, clock: Clock) -> (f64, f64, bool) {
            volume_move(
                self.volume_open,
                self.volume_from.0,
                self.volume_from.1,
                clock.since(self.volume_since, self.volume_open),
            )
        }

        /// Starts opening or closing the volume control from its current state.
        fn set_volume_open(&mut self, open: bool, clock: Clock) {
            if open == self.volume_open {
                return;
            }
            let (share, strength, _) = self.volume_now(clock);
            self.volume_from = (share, strength);
            self.volume_since = clock.now;
            self.volume_open = open;
        }

        /// The pointer entered or moved over the controls, to (`x`, `y`) on a `width` by `height`
        /// picture. Over a video the volume control opens while the pointer is on the speaker and
        /// stays open while it is on or near the open control (within 4 pt and 6 pt).
        pub fn pointer_moved(
            &mut self,
            width: f64,
            height: f64,
            x: f64,
            y: f64,
            has_audio: bool,
            clock: Clock,
        ) {
            self.inside = true;
            if self.style != Style::Video {
                return;
            }
            self.pointer = Some((x, y));
            self.last_moved = clock.now;
            let open = layout(width, height, true);
            let wanted = open.mute.contains(x, y)
                || (self.volume_open && open.volume_ground.inset(-4.0, -6.0).contains(x, y));
            self.set_volume_open(has_audio && wanted, clock);
        }

        /// The pointer left the controls, so the volume control closes.
        pub fn pointer_left(&mut self, clock: Clock) {
            self.inside = false;
            self.pointer = None;
            self.set_volume_open(false, clock);
        }

        /// The two layers' current opacities.
        fn strengths(&self, clock: Clock) -> (f64, f64) {
            let since = clock.since(self.fade_start, self.showing);
            (
                opacity(Layer::Panel, self.showing, self.fade_from.0, since),
                opacity(Layer::Buttons, self.showing, self.fade_from.1, since),
            )
        }

        /// Tracks the hover background under the pointer: a new one fades in from zero (or from its
        /// current opacity if it was fading out), and the one left fades out from its current
        /// opacity.
        fn follow_hover(&mut self, under: Option<Rect>, clock: Clock) {
            if under == self.hover.map(|ground| ground.rect) {
                return;
            }
            let strength = |ground: Option<Ground>, on: bool| {
                ground.map_or(0.0, |ground| {
                    hover_strength(on, ground.from, clock.since(ground.since, on))
                })
            };
            let returning = if under.is_some() && under == self.left.map(|ground| ground.rect) {
                strength(self.left, false)
            } else {
                0.0
            };
            if let Some(hover) = self.hover {
                self.left = Some(Ground {
                    rect: hover.rect,
                    from: strength(self.hover, true),
                    since: clock.now,
                });
            }
            self.hover = under.map(|rect| Ground {
                rect,
                from: returning,
                since: clock.now,
            });
        }

        /// The controls over a `width` by `height` picture as currently drawn, showing `shown` for
        /// a `file_bytes`-long file, with the time's two parts measured at `words`. They show over
        /// a paused picture, while the pointer or a press is on them or the menu is open, and over
        /// a playing one until the pointer rests, fading from wherever they were. `None` once the
        /// panel has faded out.
        pub fn video_frame(
            &mut self,
            width: f64,
            height: f64,
            shown: &Playback,
            file_bytes: u64,
            clock: Clock,
            words: (f64, f64),
        ) -> Option<VideoFrame> {
            let (share, strength, slider) = self.volume_now(clock);
            let laid = layout_at(width, height, share);
            let at = self.pointer.filter(|_| self.inside);
            let wanted = wanted(
                shown.playing,
                self.pointer_place(&laid, at, shown),
                (clock.now - self.last_moved) * 1000.0,
            );
            if wanted != self.showing {
                self.fade_from = self.strengths(clock);
                self.fade_start = clock.now;
                self.showing = wanted;
            }
            let (panel, buttons) = self.strengths(clock);
            if panel <= UNSEEN {
                return None;
            }
            let under = at.and_then(|(x, y)| {
                hover_ground(
                    &layout(width, height, false),
                    x,
                    y,
                    shown.has_audio,
                    &shown.menu,
                )
            });
            self.follow_hover(under, clock);
            let ground = |ground: Option<Ground>, on: bool| {
                ground
                    .map(|ground| {
                        (
                            ground.rect,
                            hover_strength(on, ground.from, clock.since(ground.since, on)),
                        )
                    })
                    .filter(|(_, strength)| *strength > UNSEEN)
            };
            if !shown.resting && shown.menu.kind() != self.menu_shown.0 {
                self.menu_shown = (shown.menu.kind(), clock.now);
            }
            let room = |button: Rect, glyph: MediaGlyph| (button.width > 0.0).then_some(glyph);
            let scrubbing = matches!(self.hold, Some(Hold::Scrub(_)));
            let preview = self.scrub_preview_at(&laid, at, width, shown, slider);
            Some(VideoFrame {
                panel,
                buttons,
                laid,
                play: room(laid.play, shown.play_glyph()),
                mute: room(laid.mute, shown.mute_glyph()).filter(|_| shown.has_audio),
                fullscreen: room(laid.fullscreen, shown.fullscreen_glyph()),
                overflow: room(laid.overflow, MediaGlyph::More),
                grounds: [ground(self.left, false), ground(self.hover, true)],
                time: time_places(&laid, words.0, words.1),
                volume: (shown.has_audio && share > 0.0).then(|| {
                    (
                        strength,
                        slider.then_some(if shown.muted {
                            0.0
                        } else {
                            f64::from(shown.volume)
                        }),
                    )
                }),
                loaded: loaded_share(file_bytes, shown.has_played),
                played: super::played_share(shown.position_seconds, shown.duration_seconds),
                knob: at.is_some_and(|(x, y)| scrubs_at(&laid, x, y, slider)) || scrubbing,
                menu: shown.menu.kind(),
                menu_since_ms: clock.since(self.menu_shown.1, true),
                preview,
            })
        }

        /// The scrub bar preview with the pointer `at`, for controls laid out as `laid`, `width`
        /// wide. It shows while scrubbing, or while in the bar's hit area (`scrubs_at`, off the
        /// volume slider when `volume_open`), with no menu open.
        fn scrub_preview_at(
            &self,
            laid: &Layout,
            at: Option<(f64, f64)>,
            width: f64,
            shown: &Playback,
            volume_open: bool,
        ) -> Option<ScrubPreview> {
            let scrubbing = matches!(self.hold, Some(Hold::Scrub(_)));
            let (x, y) = at.filter(|_| !shown.menu.is_open())?;
            if !(scrubbing || scrubs_at(laid, x, y, volume_open)) {
                return None;
            }
            scrub_preview(laid, x, width, shown.duration_seconds, self.shape?)
        }

        /// Classifies the pointer `at` for controls laid out as `laid`; it counts as on the
        /// controls while holding one or while the menu is open.
        fn pointer_place(
            &self,
            laid: &Layout,
            at: Option<(f64, f64)>,
            shown: &Playback,
        ) -> Pointer {
            if self.hold.is_some() || shown.menu.is_open() {
                return Pointer::OnControls;
            }
            let Some((x, y)) = at else {
                return Pointer::Away;
            };
            if laid.row.union(laid.timeline).contains(x, y) {
                Pointer::OnControls
            } else {
                Pointer::OverPicture
            }
        }

        /// The audio pill, `width` by `height`, with its time text measured at `clock_width`.
        pub fn sound_frame(
            &self,
            width: f64,
            height: f64,
            shown: &Playback,
            clock_width: f64,
        ) -> SoundFrame {
            let crossed = shown.muted || shown.volume == 0.0;
            SoundFrame {
                laid: super::sound_controls::layout(width, height, clock_width, shown.has_audio),
                play: if shown.playing {
                    MediaGlyph::PillPause
                } else {
                    MediaGlyph::PillPlay
                },
                played: super::played_share(shown.position_seconds, shown.duration_seconds),
                // The knob shows under the pointer and while the bar is dragged, as in Chromium.
                knob: self.inside || matches!(self.hold, Some(Hold::Scrub(_))),
                mute: shown.has_audio.then_some(if crossed {
                    MediaGlyph::PillMuted
                } else {
                    MediaGlyph::PillSound
                }),
                more: MediaGlyph::PillMore,
            }
        }

        /// The target of a press at (`x`, `y`); over a video it also counts as a pointer move. The
        /// audio pill responds only on its play button, speaker and bar (within 6 pt and 10 pt). A
        /// press on a bar holds it until `release`.
        #[allow(clippy::too_many_arguments)]
        pub fn press(
            &mut self,
            width: f64,
            height: f64,
            x: f64,
            y: f64,
            shown: &Playback,
            clock: Clock,
            clock_width: f64,
        ) -> Option<Press> {
            if shown.resting {
                return Some(Press::Wake);
            }
            let press = match self.style {
                Style::Video => {
                    self.last_moved = clock.now;
                    Some(press_at(
                        width,
                        height,
                        x,
                        y,
                        shown.has_audio,
                        self.volume_open,
                        shown.menu.is_open(),
                    ))
                }
                Style::Sound => super::sound_controls::press_at(
                    &super::sound_controls::layout(width, height, clock_width, shown.has_audio),
                    x,
                    y,
                    shown.has_audio,
                ),
            };
            self.hold = match press {
                Some(Press::Volume(bar)) => Some(Hold::Volume(bar)),
                Some(Press::Scrub(track)) => Some(Hold::Scrub(track)),
                _ => None,
            };
            press
        }

        /// The result of dragging what the press holds to `x`. Dragging the scrub bar moves its
        /// preview along.
        pub fn drag(&mut self, x: f64) -> Option<Drag> {
            match self.hold? {
                Hold::Volume(bar) => Some(Drag::Volume(volume_at(x, bar))),
                Hold::Scrub(track) => {
                    let y = self.pointer.map_or(track.y, |(_, y)| y);
                    self.pointer = Some((x, y));
                    super::share_at_pointer(x, track.x, track.width).map(Drag::Seek)
                }
            }
        }

        /// Releases the press, returning what it held.
        pub fn release(&mut self) -> Option<Hold> {
            self.hold.take()
        }

        /// Whether a press at (`x`, `y`) belongs to the player: anywhere on the audio pill, or on
        /// the video controls (`operates`).
        pub fn operates(&self, width: f64, height: f64, x: f64, y: f64, shown: &Playback) -> bool {
            match self.style {
                Style::Sound => (Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height,
                })
                .contains(x, y),
                Style::Video => {
                    operates(width, height, x, y, self.volume_open, shown.menu.is_open())
                }
            }
        }

        /// Where `key` goes; `plain` means no Command, Option or Control is held.
        pub fn route_key(&self, key: Option<Key>, plain: bool, shown: &Playback) -> KeyRoute {
            let Some(key) = key.filter(|_| plain) else {
                return KeyRoute::Pass;
            };
            if shown.resting {
                // A player at rest has no menu and no full screen for Escape to close.
                return if key == Key::Escape {
                    KeyRoute::Pass
                } else {
                    KeyRoute::Wake
                };
            }
            if self.style == Style::Video && shown.menu.is_open() {
                return KeyRoute::Menu(key);
            }
            KeyRoute::Player(key)
        }

        /// Whether the wheel at (`x`, `y`) scrolls the open speed list, which it does only over its
        /// sheet.
        pub fn wheel_scrolls(
            &self,
            width: f64,
            height: f64,
            x: f64,
            y: f64,
            shown: &Playback,
        ) -> bool {
            self.style == Style::Video
                && matches!(shown.menu, Menu::Speeds { .. })
                && shown.menu.frame(width, height).contains(x, y)
        }
    }
}

/// Chromium's `<audio controls>`, as measured: one light pill with the play button, the time,
/// the bar filling the remaining space, the speaker, and the overflow button at the right.
/// Points from the pill's top-left corner.
pub mod sound_controls {
    use super::video_controls::{Press, Rect};

    /// The size Chromium gives `<audio controls>`.
    pub const SIZE: (f64, f64) = (300.0, 54.0);
    /// Button size, vertically centred in the pill. The first button is 10 pt from the left, the
    /// overflow button 10 pt from the right with the speaker beside it.
    pub const BUTTON: f64 = 32.0;
    pub const INSET: f64 = 10.0;
    /// The pill is light regardless of page colours: its background, and the foreground for its
    /// glyphs, time and the played part of the bar, with the rest of the bar in the foreground at
    /// `TRACK_ALPHA` (sRGB, 0 to 1).
    pub const GROUND: [f64; 3] = [241.0 / 255.0, 243.0 / 255.0, 244.0 / 255.0];
    pub const INK: [f64; 3] = [0.1, 0.1, 0.1];
    pub const TRACK_ALPHA: f64 = 0.7;

    /// The pill's layout.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Layout {
        pub play: Rect,
        /// The top-left of the time's 18 pt line.
        pub time_x: f64,
        pub time_y: f64,
        pub track: Rect,
        pub mute: Rect,
        pub more: Rect,
    }

    /// The `width` by `height` pill with its time `clock_width` wide. The bar starts 17 pt after
    /// the time and ends 15.5 pt before the speaker, or before the overflow button for a silent
    /// file.
    pub fn layout(width: f64, height: f64, clock_width: f64, has_audio: bool) -> Layout {
        let middle = height / 2.0;
        let button = |x: f64| Rect {
            x,
            y: middle - BUTTON / 2.0,
            width: BUTTON,
            height: BUTTON,
        };
        let play = button(INSET);
        let mute = button(width - INSET - 2.0 * BUTTON);
        let more = button(width - INSET - BUTTON);
        let time_x = play.x + play.width + 5.0;
        let left = time_x + clock_width + 17.0;
        let right = if has_audio { mute.x } else { more.x } - 15.5;
        Layout {
            play,
            time_x,
            time_y: middle - 9.0,
            track: Rect {
                x: left,
                y: middle - 2.0,
                width: (right - left).max(1.0),
                height: 4.0,
            },
            mute,
            more,
        }
    }

    /// The target of a press at (`x`, `y`): play, the speaker, or the bar (within 6 pt and 10 pt);
    /// `None` elsewhere on the pill.
    pub fn press_at(laid: &Layout, x: f64, y: f64, has_audio: bool) -> Option<Press> {
        if laid.play.contains(x, y) {
            Some(Press::Play)
        } else if has_audio && laid.mute.contains(x, y) {
            Some(Press::Mute)
        } else if laid.track.inset(-6.0, -10.0).contains(x, y) {
            Some(Press::Scrub(laid.track))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded times and their formatted text, which the formatter must keep matching.
    const FIXTURES: &str = include_str!("player_fixtures.json");

    #[test]
    fn times_are_written_as_recorded() {
        let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).expect("fixtures");
        let cases = fixtures["timestamps"].as_array().expect("timestamps");
        assert!(cases.len() >= 20);
        for case in cases {
            let seconds = case["seconds"].as_f64().unwrap_or(f64::NAN);
            assert_eq!(
                format_timestamp(seconds),
                case["text"].as_str().expect("text"),
                "{seconds}"
            );
        }
    }

    #[test]
    fn durations_are_written_as_recorded() {
        let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).expect("fixtures");
        let cases = fixtures["durations"].as_array().expect("durations");
        assert!(cases.len() >= 10);
        for case in cases {
            let seconds = case["seconds"].as_f64().expect("seconds");
            assert_eq!(
                crate::list::format_duration(seconds),
                case["text"].as_str().expect("text"),
                "{seconds}"
            );
        }
    }

    #[test]
    fn utc_moments_read_as_javascript_writes_them() {
        let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).expect("fixtures");
        let cases = fixtures["moments"].as_array().expect("moments");
        assert!(cases.len() >= 6);
        for case in cases {
            let (seconds, text) = (
                case["seconds"].as_i64().expect("seconds"),
                case["text"].as_str().expect("text"),
            );
            assert_eq!(parse_utc(text), Some(seconds));
        }
        assert_eq!(
            parse_utc("2026-11-12T09:30:00Z"),
            parse_utc("2026-11-12T09:30:00.123Z")
        );
        assert_eq!(parse_utc("2026-11-12 09:30:00"), None);
        assert_eq!(parse_utc("2026-13-12T09:30:00Z"), None);
        assert_eq!(parse_utc(""), None);
    }

    #[test]
    fn details_follow_the_recorded_order_and_leave_out_the_unknown() {
        let fixtures: serde_json::Value = serde_json::from_str(FIXTURES).expect("fixtures");
        let labels: Vec<&str> = fixtures["detailLabels"]
            .as_array()
            .expect("labels")
            .iter()
            .filter_map(|label| label.as_str())
            .collect();
        let all = Details {
            kind: "Video".into(),
            size: Some("1.2 MB".into()),
            created: Some("Today at 9:30 AM".into()),
            modified: Some("Today at 9:31 AM".into()),
            dimensions: Some((1920, 1080)),
            duration_seconds: Some(4.04),
        };
        let lines = details_lines(&all, "en");
        assert_eq!(
            lines.iter().map(|(label, _)| *label).collect::<Vec<_>>(),
            labels
        );
        assert_eq!(
            lines[4].1,
            fixtures["dimensions"].as_str().expect("dimensions")
        );
        assert_eq!(lines[5].1, "4 s");
        let sparse = Details {
            kind: "Audio".into(),
            size: Some("--".into()),
            duration_seconds: Some(2.5),
            ..Details::default()
        };
        assert_eq!(
            details_lines(&sparse, "en"),
            vec![
                ("Kind", "Audio".to_owned()),
                ("Duration", "2.5 s".to_owned())
            ]
        );
    }

    #[test]
    fn the_summary_line_is_the_kind_and_the_size_when_there_is_one() {
        let zip = Details {
            kind: "ZIP archive".into(),
            size: Some("327 KB".into()),
            created: Some("Today".into()),
            ..Details::default()
        };
        assert_eq!(summary_line(&zip), "ZIP archive · 327 KB");
        let folder = Details {
            kind: "Folder".into(),
            created: Some("Today".into()),
            ..Details::default()
        };
        assert_eq!(
            summary_line(&folder),
            "Folder",
            "never a date in the size's place"
        );
        let unknown = Details {
            kind: "Document".into(),
            size: Some("--".into()),
            ..Details::default()
        };
        assert_eq!(summary_line(&unknown), "Document");
    }

    #[test]
    fn the_clock_and_the_bar_read_position_against_length() {
        assert_eq!(clock_text(3.9, 65.0), "0:03 / 1:05");
        assert_eq!(clock_text(0.0, f64::NAN), "0:00 / 0:00");
        assert_eq!(played_share(2.5, 5.0), 0.5);
        assert_eq!(played_share(9.0, 5.0), 1.0);
        assert_eq!(played_share(1.0, 0.0), 0.0);
    }

    #[test]
    fn a_pointer_on_the_bar_asks_for_its_share_of_the_length() {
        assert_eq!(seconds_at_pointer(150.0, 100.0, 200.0, 8.0), Some(2.0));
        assert_eq!(seconds_at_pointer(50.0, 100.0, 200.0, 8.0), Some(0.0));
        assert_eq!(seconds_at_pointer(900.0, 100.0, 200.0, 8.0), Some(8.0));
        assert_eq!(seconds_at_pointer(150.0, 100.0, 0.0, 8.0), None);
        assert_eq!(seconds_at_pointer(150.0, 100.0, 200.0, 0.0), None);
    }

    #[test]
    fn keys_do_what_the_browsers_controls_do() {
        assert_eq!(
            key_action(Key::Space, 1.0, 10.0, 1.0),
            Some(KeyAction::TogglePlaying)
        );
        assert_eq!(
            key_action(Key::Return, 1.0, 10.0, 1.0),
            Some(KeyAction::TogglePlaying)
        );
        assert_eq!(
            key_action(Key::Right, 1.0, 200.0, 1.0),
            Some(KeyAction::Seek(3.0))
        );
        assert_eq!(
            key_action(Key::Left, 1.0, 200.0, 1.0),
            Some(KeyAction::Seek(0.0))
        );
        assert_eq!(
            key_action(Key::Right, 199.5, 200.0, 1.0),
            Some(KeyAction::Seek(200.0))
        );
        assert_eq!(
            key_action(Key::Home, 7.0, 10.0, 1.0),
            Some(KeyAction::Seek(0.0))
        );
        assert_eq!(
            key_action(Key::End, 7.0, 10.0, 1.0),
            Some(KeyAction::Seek(10.0))
        );
        assert_eq!(key_action(Key::Right, 0.0, 0.0, 1.0), None);
        assert_eq!(
            key_action(Key::Down, 0.0, 10.0, 1.0),
            Some(KeyAction::Volume(0.95))
        );
        assert_eq!(
            key_action(Key::Up, 0.0, 10.0, 1.0),
            Some(KeyAction::Volume(1.0))
        );
        assert_eq!(
            key_action(Key::Down, 0.0, 10.0, 0.02),
            Some(KeyAction::Volume(0.0))
        );
        let mut volume = 1.0;
        for _ in 0..20 {
            let Some(KeyAction::Volume(next)) = key_action(Key::Down, 0.0, 10.0, volume) else {
                panic!("volume")
            };
            volume = next;
        }
        assert_eq!(volume, 0.0);
        assert_eq!(Key::from_press(crate::ffi::NrKey::Right), Some(Key::Right));
        assert_eq!(
            Key::from_press(crate::ffi::NrKey::Escape),
            Some(Key::Escape)
        );
        assert_eq!(key_action(Key::Escape, 1.0, 10.0, 1.0), None);
        assert_eq!(Key::from_press(crate::ffi::NrKey::Tab), None);
    }
    #[test]
    fn the_clock_runs_at_the_speed_chosen_and_never_jumps_when_it_changes() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        let normal = Anchor {
            at: start,
            from: 1.0,
            rate: 1.0,
        };
        assert!((normal.position(start + Duration::from_secs(2)) - 3.0).abs() < 1e-9);
        // 2 s in, at 2×: the time continues from 3 and gains 2 per second.
        let fast = normal.at_rate(start + Duration::from_secs(2), 2.0);
        assert!((fast.position(start + Duration::from_secs(2)) - 3.0).abs() < 1e-9);
        assert!((fast.position(start + Duration::from_secs(3)) - 5.0).abs() < 1e-9);
        // A frame due at 6 is 0.5 s away at that rate, and one already past is not waited for.
        assert_eq!(
            fast.until(6.0, start + Duration::from_secs(3)),
            Duration::from_millis(500)
        );
        assert_eq!(
            fast.until(4.0, start + Duration::from_secs(3)),
            Duration::ZERO
        );
        let slow = fast.at_rate(start + Duration::from_secs(3), 0.25);
        assert!((slow.position(start + Duration::from_secs(7)) - 6.0).abs() < 1e-9);
        // Reading before the anchor time gives the start.
        assert_eq!(slow.position(start), 5.0);
        assert_eq!(playback_rate(9.0), 2.0);
        assert_eq!(playback_rate(0.0), 0.25);
        assert_eq!(playback_rate(1.5), 1.5);
        assert_eq!(playback_rate(f64::NAN), 1.0);
    }

    #[test]
    fn a_button_under_the_pointer_wears_its_round_ground() {
        use super::video_controls::{button_at, hover_ground, layout, Button, Menu, Rect};
        let laid = layout(1216.0, 684.0, false);
        // As measured: the pointer at (56, 715) and (1224, 715) of the window, the picture's
        // corner at (32, 79); the grounds at 38 to 74 and 1206 to 1242, 697 to 733.
        assert_eq!(button_at(&laid, 24.0, 636.0, true), Some(Button::Play));
        assert_eq!(
            hover_ground(&laid, 24.0, 636.0, true, &Menu::Closed),
            Some(Rect {
                x: 6.0,
                y: 618.0,
                width: 36.0,
                height: 36.0
            })
        );
        assert_eq!(
            hover_ground(&laid, 1192.0, 636.0, true, &Menu::Closed),
            Some(Rect {
                x: 1174.0,
                y: 618.0,
                width: 36.0,
                height: 36.0
            })
        );
        assert_eq!(
            button_at(&laid, 1140.0, 636.0, true),
            Some(Button::Fullscreen)
        );
        // The speaker opens its volume control instead; a silent file has no speaker.
        assert_eq!(button_at(&laid, 1096.0, 636.0, true), Some(Button::Mute));
        assert_eq!(
            hover_ground(&laid, 1096.0, 636.0, true, &Menu::Closed),
            None
        );
        assert_eq!(button_at(&laid, 1096.0, 636.0, false), None);
        assert_eq!(button_at(&laid, 600.0, 636.0, true), None);
        assert_eq!(button_at(&laid, 24.0, 670.0, true), None);
        // An open menu captures the pointer.
        assert_eq!(
            hover_ground(&laid, 24.0, 636.0, true, &Menu::Main { focus: 0 }),
            None
        );
    }

    #[test]
    fn the_menu_sits_and_reads_as_chromiums_does() {
        use super::video_controls::{Menu, Rect, MAIN_ROWS};
        let (width, height) = (1216.0, 684.0);
        assert_eq!(Menu::Closed.frame(width, height), Rect::default());
        assert!(!Menu::Closed.is_open());
        let menu = Menu::Closed.toggled();
        assert_eq!(menu, Menu::Main { focus: 0 });
        // As measured: the sheet at 1044 to 1244 by 553 to 735 of the window (the picture's
        // corner at 32, 79), the first row 46 tall, the rows' middles at 576, 621, 666 and 712.
        assert_eq!(
            menu.frame(width, height),
            Rect {
                x: 1012.0,
                y: 474.0,
                width: 200.0,
                height: 182.0
            }
        );
        let middles: Vec<f64> = (0..4)
            .map(|index| {
                menu.row(index, width, height)
                    .map_or(0.0, |row| row.y + row.height / 2.0 + 79.0)
            })
            .collect();
        assert_eq!(middles, [576.0, 621.0, 666.0, 712.0]);
        assert_eq!(menu.row(4, width, height), None);
        assert_eq!(
            (0..4)
                .map(|index| menu.label(index, false, "en"))
                .collect::<Vec<_>>(),
            [
                Some("Download"),
                Some("Captions"),
                Some("Playback speed"),
                Some("Picture in Picture")
            ]
        );
        assert_eq!(
            (
                menu.label(1, true, "en"),
                menu.label(0, true, "en"),
                menu.label(4, false, "en")
            ),
            (Some("Off"), None, None)
        );
        let speeds = Menu::Speeds {
            focus: 0,
            scroll: 0.0,
        };
        assert_eq!(
            (1..=8)
                .map(|index| speeds.label(index, false, "en").unwrap_or(""))
                .collect::<Vec<_>>(),
            ["0.25", "0.5", "0.75", "Normal", "1.25", "1.5", "1.75", "2"]
        );
        assert_eq!(
            (
                speeds.label(0, false, "en"),
                speeds.label(9, false, "en"),
                speeds.label(1, true, "en")
            ),
            (Some("Playback speed"), None, None)
        );
        assert_eq!(MAIN_ROWS.len(), 4);
        // A point on "Playback speed", as measured: (1148, 665) of the window.
        assert_eq!(
            menu.row_at(1148.0 - 32.0, 665.0 - 79.0, width, height),
            Some(2)
        );
        assert_eq!(menu.row_at(1000.0, 500.0, width, height), None);
        assert_eq!(menu.toggled(), Menu::Closed);
        // A picture too short for the sheet shows what fits of it.
        assert_eq!(Menu::Main { focus: 0 }.frame(448.0, 150.0).height, 118.0);
    }

    #[test]
    fn the_list_of_speeds_opens_on_the_ticked_one() {
        use super::video_controls::{ticked_speed, Chosen, Menu, Rect};
        let (width, height) = (1216.0, 684.0);
        let (list, chosen) =
            Menu::Main { focus: 0 }.pressed(1148.0 - 32.0, 665.0 - 79.0, 1.0, width, height);
        assert_eq!(chosen, Chosen::Nothing);
        // As measured: the sheet 250 tall from 485 of the window, "Normal" in focus across
        // its middle (586 to 634), "0.5" starting 5 under the sheet's top.
        assert_eq!(
            list,
            Menu::Speeds {
                focus: 4,
                scroll: 91.0
            }
        );
        assert_eq!(
            list.frame(width, height),
            Rect {
                x: 1012.0,
                y: 406.0,
                width: 200.0,
                height: 250.0
            }
        );
        assert_eq!(
            list.row(4, width, height),
            Some(Rect {
                x: 1012.0,
                y: 507.0,
                width: 200.0,
                height: 48.0
            })
        );
        assert_eq!(list.row(2, width, height).map(|row| row.y), Some(411.0));
        assert_eq!(list.rows(), 9);
        assert_eq!(
            (
                ticked_speed(1.0),
                ticked_speed(0.25),
                ticked_speed(2.0),
                ticked_speed(1.3)
            ),
            (3, 0, 7, 4)
        );
        // At the ends the list cannot be scrolled far enough to centre the ticked speed.
        assert_eq!(
            Menu::speeds(0.25, width, height),
            Menu::Speeds {
                focus: 1,
                scroll: 0.0
            }
        );
        assert_eq!(
            Menu::speeds(2.0, width, height),
            Menu::Speeds {
                focus: 8,
                scroll: 182.0
            }
        );
        // A wheel moves it between its ends, and only it.
        assert_eq!(
            list.scrolled(-500.0, width, height),
            Menu::Speeds {
                focus: 4,
                scroll: 0.0
            }
        );
        assert_eq!(
            list.scrolled(500.0, width, height),
            Menu::Speeds {
                focus: 4,
                scroll: 182.0
            }
        );
        assert_eq!(
            Menu::Main { focus: 1 }.scrolled(40.0, width, height),
            Menu::Main { focus: 1 }
        );
    }

    #[test]
    fn a_press_or_a_key_on_the_menu_does_what_its_row_does() {
        use super::video_controls::{Chosen, Menu};
        let (width, height) = (1216.0, 684.0);
        let main = Menu::Main { focus: 0 };
        let at = |menu: Menu, row: usize| {
            let rect = menu.row(row, width, height).expect("row");
            menu.pressed(
                rect.x + 100.0,
                rect.y + rect.height / 2.0,
                1.0,
                width,
                height,
            )
        };
        assert_eq!(at(main, 0), (Menu::Closed, Chosen::Download));
        // Captions has nothing to list: the menu stays, on that row.
        assert_eq!(at(main, 1), (Menu::Main { focus: 1 }, Chosen::Captions));
        assert_eq!(at(main, 3), (Menu::Closed, Chosen::PictureInPicture));
        // A press off the sheet closes it and does nothing else.
        assert_eq!(
            main.pressed(10.0, 10.0, 1.0, width, height),
            (Menu::Closed, Chosen::Nothing)
        );
        let list = Menu::speeds(1.0, width, height);
        assert_eq!(at(list, 6), (Menu::Closed, Chosen::Rate(1.5)));
        assert_eq!(at(list, 2), (Menu::Closed, Chosen::Rate(0.5)));
        // Rows scrolled out of the sheet take no press: the point is off the sheet.
        assert_eq!(
            list.row_at(
                1100.0,
                list.row(0, width, height).expect("heading").y + 10.0,
                width,
                height
            ),
            None
        );
        // The heading goes back to the menu, on the row the list was opened from.
        let top = list.scrolled(-500.0, width, height);
        assert_eq!(at(top, 0), (Menu::Main { focus: 2 }, Chosen::Nothing));
        // Keys: Down and Up move the focus and stop at the ends, Return does the row, Escape closes.
        let key = |menu: Menu, key: Key| menu.key(key, 1.0, width, height).expect("the menu's key");
        assert_eq!(key(main, Key::Up).0, Menu::Main { focus: 0 });
        assert_eq!(
            key(key(main, Key::Down).0, Key::Down).0,
            Menu::Main { focus: 2 }
        );
        assert_eq!(
            key(Menu::Main { focus: 3 }, Key::Down).0,
            Menu::Main { focus: 3 }
        );
        assert_eq!(key(Menu::Main { focus: 2 }, Key::Return).0, list);
        assert_eq!(key(list, Key::Return), (Menu::Closed, Chosen::Rate(1.0)));
        assert_eq!(key(list, Key::Escape), (Menu::Closed, Chosen::Nothing));
        assert_eq!(main.key(Key::Left, 1.0, width, height), None);
        assert_eq!(Menu::Closed.key(Key::Down, 1.0, width, height), None);
        // The list follows the focus to its ends: "2" is brought wholly into the sheet.
        let mut walked = list;
        for _ in 0..6 {
            walked = key(walked, Key::Down).0;
        }
        assert_eq!(
            walked,
            Menu::Speeds {
                focus: 8,
                scroll: 182.0
            }
        );
        for _ in 0..9 {
            walked = key(walked, Key::Up).0;
        }
        assert_eq!(
            walked,
            Menu::Speeds {
                focus: 0,
                scroll: 0.0
            }
        );
    }

    #[test]
    fn full_screen_is_entered_by_its_button_and_left_by_it_or_escape() {
        use super::video_controls::{fullscreen_after, fullscreen_fit, FullscreenAsk, Rect};
        assert!(fullscreen_after(false, FullscreenAsk::Toggle));
        assert!(!fullscreen_after(true, FullscreenAsk::Toggle));
        assert!(!fullscreen_after(true, FullscreenAsk::Leave));
        assert!(!fullscreen_after(false, FullscreenAsk::Leave));
        // A 16:9 picture on a 16:10 screen: the whole width, bars above and below.
        let wide = fullscreen_fit(1280.0, 720.0, 1728.0, 1080.0);
        assert_eq!((wide.x, wide.width), (0.0, 1728.0));
        assert!(
            (wide.y - 54.0).abs() < 1e-9 && (wide.height - 972.0).abs() < 1e-9,
            "{wide:?}"
        );
        // A tall picture: the whole height, bars at the sides; a small one is enlarged.
        let tall = fullscreen_fit(360.0, 640.0, 1920.0, 1080.0);
        assert_eq!((tall.y, tall.height), (0.0, 1080.0));
        assert!(
            (tall.x - 656.25).abs() < 1e-9 && (tall.width - 607.5).abs() < 1e-9,
            "{tall:?}"
        );
        assert_eq!(
            fullscreen_fit(0.0, 0.0, 800.0, 600.0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 800.0,
                height: 600.0
            }
        );
    }

    #[test]
    fn the_controls_sit_where_chromiums_do() {
        use super::video_controls::{layout, Rect};
        // The video that fills a 1280 by 800 window: 1216 by 684 (the measured boxes, less the
        // picture's corner at 32, 79).
        let laid = layout(1216.0, 684.0, false);
        assert_eq!(
            laid.play,
            Rect {
                x: 0.0,
                y: 612.0,
                width: 48.0,
                height: 48.0
            }
        );
        assert_eq!(laid.time_x, 48.0);
        assert_eq!(
            laid.mute,
            Rect {
                x: 1072.0,
                y: 612.0,
                width: 48.0,
                height: 48.0
            }
        );
        assert_eq!(laid.fullscreen.x, 1120.0);
        assert_eq!(laid.overflow.x, 1168.0);
        assert_eq!(
            laid.track,
            Rect {
                x: 16.0,
                y: 660.0,
                width: 1184.0,
                height: 4.0
            }
        );
        assert_eq!(
            laid.shade,
            Rect {
                x: 0.0,
                y: 572.0,
                width: 1216.0,
                height: 112.0
            }
        );
        assert_eq!(laid.volume_bar, Rect::default());
        // The pointer on the speaker: the slider opens to its left on a ground of its own.
        let open = layout(1216.0, 684.0, true);
        assert_eq!(
            open.volume_ground,
            Rect {
                x: 1004.0,
                y: 618.0,
                width: 112.0,
                height: 36.0
            }
        );
        assert_eq!(
            open.volume_bar,
            Rect {
                x: 1020.0,
                y: 634.0,
                width: 52.0,
                height: 4.0
            }
        );
        // The video beside the columns, 448 by 252.
        let small = layout(448.0, 252.0, false);
        assert_eq!(
            (small.play.y, small.mute.x, small.track.width, small.track.y),
            (180.0, 304.0, 416.0, 228.0)
        );
    }

    #[test]
    fn a_narrow_picture_drops_buttons_in_chromiums_order() {
        use super::video_controls::{
            button_at, buttons_that_fit, hover_ground, layout, Button, ButtonsThatFit, Menu, Rect,
        };
        let fit = |play, mute, fullscreen, overflow| ButtonsThatFit {
            play,
            mute,
            fullscreen,
            overflow,
        };
        // The overflow button is reserved first; then play, the speaker and full screen while each
        // fits.
        assert_eq!(buttons_that_fit(1216.0), fit(true, true, true, true));
        assert_eq!(buttons_that_fit(192.0), fit(true, true, true, true));
        assert_eq!(buttons_that_fit(191.0), fit(true, true, false, true));
        assert_eq!(buttons_that_fit(144.0), fit(true, true, false, true));
        assert_eq!(buttons_that_fit(143.0), fit(true, false, false, true));
        assert_eq!(buttons_that_fit(96.0), fit(true, false, false, true));
        assert_eq!(buttons_that_fit(95.0), fit(false, false, false, true));
        assert_eq!(buttons_that_fit(47.0), fit(false, false, false, false));
        // 240 and 200 across: every button, in its place.
        for width in [240.0, 200.0] {
            let laid = layout(width, 135.0, false);
            assert_eq!(
                (laid.play.x, laid.mute.x, laid.fullscreen.x, laid.overflow.x),
                (0.0, width - 144.0, width - 96.0, width - 48.0)
            );
        }
        // 160 across: full screen goes, and the speaker moves up beside the dots.
        let laid = layout(160.0, 90.0, false);
        assert_eq!(laid.fullscreen, Rect::default());
        assert_eq!(
            (laid.play.x, laid.mute.x, laid.overflow.x),
            (0.0, 64.0, 112.0)
        );
        let row = laid.play.y + 24.0;
        assert_eq!(button_at(&laid, 70.0, row, true), Some(Button::Mute));
        assert_eq!(button_at(&laid, 120.0, row, true), Some(Button::Overflow));
        // 120 across: only play and the overflow button; nothing responds where the speaker was,
        // and its volume control never opens.
        let laid = layout(120.0, 68.0, true);
        assert_eq!(
            (laid.mute, laid.volume_ground, laid.volume_bar),
            (Rect::default(), Rect::default(), Rect::default())
        );
        assert_eq!(button_at(&laid, 60.0, laid.play.y + 24.0, true), None);
        assert_eq!(
            hover_ground(&laid, 60.0, laid.play.y + 24.0, true, &Menu::Closed),
            None
        );
        // The scrub bar keeps its row at every width.
        assert_eq!(
            laid.track,
            Rect {
                x: 16.0,
                y: 44.0,
                width: 88.0,
                height: 4.0
            }
        );
    }

    #[test]
    fn the_loaded_part_is_what_chromium_reads_before_playing() {
        use super::video_controls::loaded_share;
        // As measured: a 1,617,898-byte clip showed 815 of its bar's 1184 points loaded.
        let share = loaded_share(1_617_898, false);
        assert!((share * 1184.0 - 815.3).abs() < 0.1, "{share}");
        assert_eq!(loaded_share(1_617_898, true), 1.0);
        assert_eq!(loaded_share(200_000, false), 1.0);
    }

    #[test]
    fn the_controls_hide_over_a_playing_picture_left_alone() {
        use super::video_controls::{opacity, wanted, Pointer};
        assert!(wanted(false, Pointer::Away, 60_000.0));
        assert!(wanted(true, Pointer::OverPicture, 2_999.0));
        assert!(!wanted(true, Pointer::OverPicture, 3_000.0));
        assert!(wanted(true, Pointer::OnControls, 60_000.0));
        assert!(!wanted(true, Pointer::Away, 0.0));
        // The panel fades out over 1 s and in over 250 ms, the buttons out over 750 ms and in
        // over 500 ms, on `ease`, from wherever they were. The values were read from Chromium's
        // controls frozen 250 and 500 ms into hiding.
        use super::video_controls::Layer::{Buttons, Panel};
        assert_eq!(opacity(Panel, false, 1.0, 0.0), 1.0);
        assert_eq!(opacity(Panel, false, 1.0, 1000.0), 0.0);
        assert!(
            (opacity(Panel, false, 1.0, 250.0) - 0.591489).abs() < 1e-4,
            "{}",
            opacity(Panel, false, 1.0, 250.0)
        );
        assert!((opacity(Panel, false, 1.0, 500.0) - 0.197597).abs() < 1e-4);
        assert!((opacity(Buttons, false, 1.0, 250.0) - 0.424138).abs() < 1e-4);
        assert!((opacity(Buttons, false, 1.0, 500.0) - 0.075176).abs() < 1e-4);
        assert_eq!(opacity(Panel, true, 0.0, 250.0), 1.0);
        assert_eq!(opacity(Buttons, true, 0.0, 500.0), 1.0);
        assert!((opacity(Panel, true, 0.4, 0.0) - 0.4).abs() < 1e-9);
        assert!(opacity(Panel, true, 0.0, 125.0) > 0.7);
    }

    /// Animations read from Chromium's controls frozen part-way: the volume control opening, the
    /// menu growing and its rows fading in.
    #[test]
    fn the_controls_move_as_chromiums_held_part_way() {
        use super::video_controls::{layout_at, menu_entrance, menu_row_entrance, volume_move};
        let near = |a: f64, b: f64| (a - b).abs() < 2e-3;
        for (ms, ground, bar, strength) in [
            (50.0, 52.7656, 11.4688, 0.209623),
            (100.0, 79.7578, 29.9375, 0.484604),
            (150.0, 96.9766, 41.7188, 0.628513),
            (250.0, 110.742, 51.1406, 0.71),
        ] {
            let (share, alpha, slider) = volume_move(true, 0.0, 0.0, ms);
            let laid = layout_at(1216.0, 684.0, share);
            assert!(slider);
            assert!(
                (laid.volume_ground.width - ground).abs() < 0.02,
                "{ms}: ground {}",
                laid.volume_ground.width
            );
            assert!(
                (laid.volume_bar.width - bar).abs() < 0.02,
                "{ms}: bar {}",
                laid.volume_bar.width
            );
            assert!(near(alpha, strength), "{ms}: strength {alpha}");
            // Right edges stay: the background 4 pt short of the speaker's, the bar at the speaker.
            assert_eq!(
                laid.volume_ground.x + laid.volume_ground.width,
                laid.mute.x + 44.0
            );
            assert_eq!(laid.volume_bar.x + laid.volume_bar.width, laid.mute.x);
        }
        assert!(volume_move(false, 1.0, 0.71, 279.0).2 && !volume_move(false, 1.0, 0.71, 280.0).2);
        for (ms, scale, strength) in [
            (100.0, 0.488349, 0.5),
            (150.0, 0.684643, 0.75),
            (200.0, 0.843836, 1.0),
            (300.0, 1.0, 1.0),
        ] {
            let (size, alpha) = menu_entrance(ms);
            assert!(
                (size - scale).abs() < 1e-4 && near(alpha, strength),
                "{ms}: {size} {alpha}"
            );
        }
        // Rows from the bottom: (opacity, offset below) at 150, 200 and 300 ms.
        for (row, ms, strength, below) in [
            (0, 150.0, 0.0259846, 0.0),
            (0, 300.0, 0.315357, 0.0),
            (1, 200.0, 0.136888, 8.87234),
            (1, 300.0, 0.645321, 0.593115),
            (2, 300.0, 0.408511, 2.96395),
            (3, 300.0, 0.136888, 8.87234),
            (3, 450.0, 0.802403, 0.0),
        ] {
            let (alpha, offset) = menu_row_entrance(row, ms);
            assert!(
                (alpha - strength).abs() < 1e-4 && (offset - below).abs() < 1e-3,
                "row {row} at {ms}: {alpha} {offset}"
            );
        }
    }

    #[test]
    fn a_press_on_the_volume_bar_sets_the_volume() {
        use super::video_controls::{layout, volume_at};
        let bar = layout(1216.0, 684.0, true).volume_bar;
        assert_eq!(volume_at(bar.x, bar), 0.0);
        assert_eq!(volume_at(bar.x + 26.0, bar), 0.5);
        assert_eq!(volume_at(bar.x + 400.0, bar), 1.0);
    }

    fn playback() -> video_controls::Playback {
        video_controls::Playback {
            playing: false,
            position_seconds: 1.0,
            duration_seconds: 4.0,
            has_audio: true,
            muted: false,
            volume: 1.0,
            fullscreen: false,
            has_played: false,
            menu: video_controls::Menu::Closed,
            resting: false,
        }
    }

    fn at(now: f64) -> video_controls::Clock {
        video_controls::Clock {
            now,
            entrance: None,
            exit: None,
        }
    }

    #[test]
    fn a_press_goes_to_the_control_under_it_or_the_picture() {
        use super::video_controls::{layout, operates, press_at, Press};
        let (width, height) = (1216.0, 684.0);
        let laid = layout(width, height, true);
        let middle =
            |rect: video_controls::Rect| (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0);
        let press = |(x, y): (f64, f64), volume_open: bool| {
            press_at(width, height, x, y, true, volume_open, false)
        };
        assert_eq!(press(middle(laid.overflow), false), Press::ToggleMenu);
        assert_eq!(press(middle(laid.fullscreen), false), Press::Fullscreen);
        assert_eq!(press(middle(laid.play), false), Press::Play);
        assert_eq!(press(middle(laid.mute), false), Press::Mute);
        // The open slider, off the speaker, takes the press; closed, the same point is the picture.
        let slider = (laid.volume_bar.x + 10.0, laid.volume_bar.y);
        assert_eq!(press(slider, true), Press::Volume(laid.volume_bar));
        assert_eq!(press(slider, false), Press::Picture);
        assert_eq!(press(middle(laid.mute), true), Press::Mute);
        // The slider's foot, where it reaches over the scrub bar's band, is still the slider's.
        let foot = (
            laid.volume_bar.x + 10.0,
            laid.volume_ground.y + laid.volume_ground.height - 1.0,
        );
        assert!(video_controls::scrub_band(&laid).contains(foot.0, foot.1));
        assert_eq!(press(foot, true), Press::Volume(laid.volume_bar));
        // The bar's row and 12 pt above it, even under a button's glyph; above that, the button
        // row or the picture.
        assert_eq!(
            press((600.0, laid.timeline.y - 11.0), false),
            Press::Scrub(laid.track)
        );
        let under_play = (
            laid.play.x + video_controls::BUTTON / 2.0,
            laid.timeline.y - 6.0,
        );
        assert_eq!(press(under_play, false), Press::Scrub(laid.track));
        assert_eq!(
            press((600.0, laid.timeline.y - 13.0), false),
            Press::Picture
        );
        assert_eq!(press((600.0, 100.0), false), Press::Picture);
        // An open menu captures every press; a file without sound has no speaker.
        assert_eq!(
            press_at(width, height, 600.0, 100.0, true, false, true),
            Press::Menu
        );
        let (x, y) = middle(laid.mute);
        assert_eq!(
            press_at(width, height, x, y, false, false, false),
            Press::Picture
        );
        // Which presses belong to the controls and which to the picture.
        assert!(operates(width, height, x, y, false, false));
        assert!(operates(
            width,
            height,
            600.0,
            laid.timeline.y - 3.0,
            false,
            false
        ));
        assert!(!operates(
            width,
            height,
            600.0,
            laid.timeline.y + laid.timeline.height + 3.0,
            false,
            false
        ));
        assert!(!operates(width, height, 600.0, 100.0, false, false));
        assert!(operates(width, height, 600.0, 100.0, false, true));
        assert!(!operates(width, height, -1.0, 100.0, false, true));
    }

    #[test]
    fn the_time_is_written_only_where_it_fits() {
        use super::video_controls::{layout, time_places};
        let laid = layout(1216.0, 684.0, false);
        // Chromium's "0:00" and "/ 0:04" in 14 px Roboto.
        assert_eq!(
            time_places(&laid, 28.3, 38.1),
            (Some(48.0), Some(48.0 + 29.0 + 4.0))
        );
        // Narrow: the second part goes first, then both, and with a button gone neither shows.
        let narrow = layout(200.0, 112.0, false);
        assert_eq!(time_places(&narrow, 1.5, 2.0), (Some(48.0), Some(54.0)));
        assert_eq!(time_places(&narrow, 1.5, 2.5), (Some(48.0), None));
        assert_eq!(time_places(&narrow, 9.0, 0.0), (None, None));
        assert_eq!(
            time_places(&layout(191.0, 112.0, false), 1.0, 1.0),
            (None, None)
        );
        assert_eq!(
            clock_parts(3.9, 65.0),
            ("0:03".to_owned(), "/ 1:05".to_owned())
        );
    }

    #[test]
    fn the_sound_pill_sits_as_the_browsers_does() {
        use super::sound_controls::{layout, press_at, SIZE};
        use super::video_controls::{Press, Rect};
        let (width, height) = SIZE;
        let laid = layout(width, height, 70.0, true);
        assert_eq!(
            laid.play,
            Rect {
                x: 10.0,
                y: 11.0,
                width: 32.0,
                height: 32.0
            }
        );
        assert_eq!((laid.time_x, laid.time_y), (47.0, 18.0));
        assert_eq!(laid.mute.x, 226.0);
        assert_eq!(laid.more.x, 258.0);
        assert_eq!(
            laid.track,
            Rect {
                x: 134.0,
                y: 25.0,
                width: 76.5,
                height: 4.0
            }
        );
        // Without sound the bar runs on to the dots.
        assert_eq!(layout(width, height, 70.0, false).track.width, 108.5);
        // A time too wide still leaves a 1 pt bar.
        assert_eq!(layout(width, height, 400.0, true).track.width, 1.0);
        assert_eq!(press_at(&laid, 20.0, 27.0, true), Some(Press::Play));
        assert_eq!(press_at(&laid, 230.0, 27.0, true), Some(Press::Mute));
        assert_eq!(press_at(&laid, 230.0, 27.0, false), None);
        assert_eq!(
            press_at(&laid, 130.0, 16.0, true),
            Some(Press::Scrub(laid.track))
        );
        assert_eq!(press_at(&laid, 127.0, 27.0, true), None);
        assert_eq!(press_at(&laid, 270.0, 27.0, true), None);
    }

    #[test]
    fn the_volume_control_opens_on_the_speaker_and_carries_on_from_where_it_was() {
        use super::video_controls::{layout, Controls, Style};
        let (width, height) = (1216.0, 684.0);
        let mut controls = Controls::new(Style::Video, 100.0);
        let shown = playback();
        let speaker = layout(width, height, false).mute;
        let frame = |controls: &mut Controls, now: f64| {
            controls
                .video_frame(width, height, &shown, 0, at(now), (0.0, 0.0))
                .expect("drawn")
        };
        controls.pointer_moved(
            width,
            height,
            speaker.x + 24.0,
            speaker.y + 24.0,
            true,
            at(100.0),
        );
        let open = frame(&mut controls, 100.15);
        let (strength, slider) = open.volume.expect("open");
        assert!(strength > 0.5 && slider == Some(1.0), "{strength}");
        assert!(open.laid.volume_ground.width > 90.0);
        // Over the open slider it stays open; off it, it closes from its current state.
        controls.pointer_moved(
            width,
            height,
            open.laid.volume_bar.x + 5.0,
            speaker.y + 24.0,
            true,
            at(100.2),
        );
        assert!(frame(&mut controls, 100.5)
            .volume
            .is_some_and(|(strength, _)| (strength - 0.71).abs() < 1e-9));
        controls.pointer_moved(width, height, 600.0, 100.0, true, at(100.5));
        let closing = frame(&mut controls, 100.6);
        assert!(
            closing.laid.volume_ground.width < 112.0 && closing.laid.volume_ground.width > 36.0
        );
        // Back on the speaker part-way through closing: it reopens from there, not from closed.
        controls.pointer_moved(
            width,
            height,
            speaker.x + 24.0,
            speaker.y + 24.0,
            true,
            at(100.6),
        );
        assert!(
            (frame(&mut controls, 100.6).laid.volume_ground.width
                - closing.laid.volume_ground.width)
                .abs()
                < 1e-9
        );
        // A file without sound never opens it; leaving the controls closes it.
        let mut silent = Controls::new(Style::Video, 100.0);
        silent.pointer_moved(
            width,
            height,
            speaker.x + 24.0,
            speaker.y + 24.0,
            false,
            at(100.0),
        );
        assert!(frame(&mut silent, 101.0).volume.is_none());
        controls.pointer_left(at(101.0));
        assert!(frame(&mut controls, 102.0).volume.is_none());
    }

    #[test]
    fn the_controls_fade_from_the_strength_they_had() {
        use super::video_controls::{Controls, Style};
        let (width, height) = (1216.0, 684.0);
        let mut controls = Controls::new(Style::Video, 100.0);
        let playing = video_controls::Playback {
            playing: true,
            ..playback()
        };
        // Paused they show; playing with the pointer away they fade over 1 s.
        assert_eq!(
            controls
                .video_frame(width, height, &playback(), 0, at(100.0), (0.0, 0.0))
                .map(|frame| frame.panel),
            Some(1.0)
        );
        let half = controls
            .video_frame(width, height, &playing, 0, at(100.25), (0.0, 0.0))
            .map(|frame| frame.panel);
        assert_eq!(half, Some(1.0), "the fade starts at this frame");
        let fading = controls
            .video_frame(width, height, &playing, 0, at(100.5), (0.0, 0.0))
            .expect("fading")
            .panel;
        assert!(fading < 0.7 && fading > 0.1, "{fading}");
        assert_eq!(
            controls.video_frame(width, height, &playing, 0, at(101.5), (0.0, 0.0)),
            None
        );
        // Paused again part-way: it returns from where it was, over 250 ms.
        let mut again = Controls::new(Style::Video, 100.0);
        again.video_frame(width, height, &playing, 0, at(100.0), (0.0, 0.0));
        let was = again
            .video_frame(width, height, &playing, 0, at(100.25), (0.0, 0.0))
            .expect("fading")
            .panel;
        let back = again
            .video_frame(width, height, &playback(), 0, at(100.25), (0.0, 0.0))
            .expect("back")
            .panel;
        assert!((back - was).abs() < 1e-9);
        assert_eq!(
            again
                .video_frame(width, height, &playback(), 0, at(100.5), (0.0, 0.0))
                .map(|frame| frame.panel),
            Some(1.0)
        );
        // The pointer resting on a playing picture hides them after 3 s; on the controls, never.
        let mut resting = Controls::new(Style::Video, 100.0);
        resting.pointer_moved(width, height, 600.0, 100.0, true, at(100.0));
        assert!(resting
            .video_frame(width, height, &playing, 0, at(102.9), (0.0, 0.0))
            .is_some_and(|frame| frame.panel == 1.0));
        resting.video_frame(width, height, &playing, 0, at(103.1), (0.0, 0.0));
        assert_eq!(
            resting.video_frame(width, height, &playing, 0, at(104.2), (0.0, 0.0)),
            None
        );
        resting.pointer_moved(width, height, 600.0, 640.0, true, at(105.0));
        resting.video_frame(width, height, &playing, 0, at(105.0), (0.0, 0.0));
        assert!(resting
            .video_frame(width, height, &playing, 0, at(120.0), (0.0, 0.0))
            .is_some_and(|frame| frame.panel == 1.0));
    }

    #[test]
    fn a_round_ground_fades_in_under_the_pointer_and_out_behind_it() {
        use super::video_controls::{layout, Controls, Style};
        let (width, height) = (1216.0, 684.0);
        let laid = layout(width, height, false);
        let mut controls = Controls::new(Style::Video, 100.0);
        let frame = |controls: &mut Controls, now: f64| {
            controls
                .video_frame(width, height, &playback(), 0, at(now), (0.0, 0.0))
                .expect("drawn")
        };
        controls.pointer_moved(
            width,
            height,
            laid.play.x + 24.0,
            laid.play.y + 24.0,
            true,
            at(100.0),
        );
        // It fades in from zero, which is not drawn.
        assert_eq!(frame(&mut controls, 100.0).grounds, [None, None]);
        let (rect, lit) = frame(&mut controls, 100.1).grounds[1].expect("lit");
        assert_eq!((rect.x, rect.width), (6.0, 36.0));
        assert!(lit > 0.2 && lit < 0.71, "{lit}");
        // Moving to the picture fades it out; moving back fades it in from its current opacity.
        controls.pointer_moved(width, height, 600.0, 100.0, true, at(100.1));
        let left = frame(&mut controls, 100.1);
        assert_eq!(left.grounds[1], None);
        assert!((left.grounds[0].expect("leaving").1 - lit).abs() < 1e-9);
        let fading = frame(&mut controls, 100.15).grounds[0].expect("leaving").1;
        controls.pointer_moved(
            width,
            height,
            laid.play.x + 24.0,
            laid.play.y + 24.0,
            true,
            at(100.15),
        );
        let returned = frame(&mut controls, 100.15).grounds[1].expect("back").1;
        assert!((returned - fading).abs() < 1e-9 && returned > 0.0);
        // The speaker opens its volume control instead.
        controls.pointer_moved(
            width,
            height,
            laid.mute.x + 24.0,
            laid.mute.y + 24.0,
            true,
            at(101.0),
        );
        assert_eq!(frame(&mut controls, 101.0).grounds[1], None);
    }

    #[test]
    fn a_file_played_is_drawn_as_all_loaded_and_each_opening_starts_again() {
        use super::video_controls::{Controls, Style};
        let mut controls = Controls::new(Style::Video, 100.0);
        let fresh = controls
            .video_frame(1216.0, 684.0, &playback(), 1_617_898, at(100.0), (0.0, 0.0))
            .expect("drawn");
        assert!(fresh.loaded < 0.7 && fresh.played == 0.25);
        let played = video_controls::Playback {
            has_played: true,
            ..playback()
        };
        assert_eq!(
            controls
                .video_frame(1216.0, 684.0, &played, 1_617_898, at(100.0), (0.0, 0.0))
                .map(|frame| frame.loaded),
            Some(1.0)
        );
        // The same controls showing a resting page player, or the file reopened.
        let resting = video_controls::Playback {
            resting: true,
            ..playback()
        };
        assert!(controls
            .video_frame(1216.0, 684.0, &resting, 1_617_898, at(100.0), (0.0, 0.0))
            .is_some_and(|frame| frame.loaded < 0.7));
    }

    #[test]
    fn the_glyphs_follow_the_playback_and_the_room() {
        use super::video_controls::{Controls, Style};
        use crate::glyph::MediaGlyph;
        let mut controls = Controls::new(Style::Video, 100.0);
        let shown = video_controls::Playback {
            playing: true,
            muted: true,
            fullscreen: true,
            ..playback()
        };
        let frame = controls
            .video_frame(1216.0, 684.0, &shown, 0, at(100.0), (0.0, 0.0))
            .expect("drawn");
        assert_eq!(
            (frame.play, frame.mute, frame.fullscreen, frame.overflow),
            (
                Some(MediaGlyph::Pause),
                Some(MediaGlyph::Muted),
                Some(MediaGlyph::LeaveFullscreen),
                Some(MediaGlyph::More)
            )
        );
        let quiet = video_controls::Playback {
            volume: 0.0,
            ..playback()
        };
        assert_eq!(
            controls
                .video_frame(1216.0, 684.0, &quiet, 0, at(100.0), (0.0, 0.0))
                .and_then(|frame| frame.mute),
            Some(MediaGlyph::Muted)
        );
        let silent = video_controls::Playback {
            has_audio: false,
            ..playback()
        };
        assert_eq!(
            controls
                .video_frame(1216.0, 684.0, &silent, 0, at(100.0), (0.0, 0.0))
                .and_then(|frame| frame.mute),
            None
        );
        // 120 across: only play and the overflow button.
        let narrow = controls
            .video_frame(120.0, 68.0, &playback(), 0, at(100.0), (0.0, 0.0))
            .expect("drawn");
        assert_eq!(
            (narrow.play, narrow.mute, narrow.fullscreen, narrow.overflow),
            (Some(MediaGlyph::Play), None, None, Some(MediaGlyph::More))
        );
        assert_eq!(
            video_controls::Menu::Main { focus: 0 }.row_glyph(2),
            Some(MediaGlyph::Speed)
        );
        assert_eq!(
            video_controls::Menu::speeds(1.0, 1216.0, 684.0).row_glyph(0),
            Some(MediaGlyph::Back)
        );
        assert_eq!(
            video_controls::Menu::speeds(1.0, 1216.0, 684.0).row_glyph(1),
            None
        );
    }

    #[test]
    fn the_scrub_preview_fits_the_videos_shape_above_the_bar_inside_the_picture() {
        use super::video_controls::{layout, scrub_preview, Rect, PREVIEW_EDGE};
        let (width, height) = (1216.0, 684.0);
        let laid = layout(width, height, false);
        let bottom = laid.track.y - 8.0;
        let middle = laid.track.x + laid.track.width / 2.0;
        let wide = scrub_preview(&laid, middle, width, 200.0, (1920.0, 1080.0)).unwrap();
        assert_eq!(
            wide.picture,
            Rect {
                x: (middle - 80.0).round(),
                y: bottom - 90.0,
                width: 160.0,
                height: 90.0,
            }
        );
        assert_eq!(
            wide.line,
            Rect {
                y: bottom - 22.0,
                height: 18.0,
                ..wide.picture
            }
        );
        assert_eq!(wide.at_seconds, 100.0);
        assert_eq!(wide.moment_seconds, 100.0);
        // A tall video is limited by the preview's height.
        let tall = scrub_preview(&laid, middle, width, 200.0, (1080.0, 1920.0)).unwrap();
        assert_eq!((tall.picture.width, tall.picture.height), (67.0, 120.0));
        // At the ends it stays inside the picture; past the end it shows the last step.
        let start = scrub_preview(&laid, 0.0, width, 200.0, (16.0, 9.0)).unwrap();
        assert_eq!(start.picture.x, PREVIEW_EDGE);
        assert_eq!((start.at_seconds, start.moment_seconds), (0.0, 0.0));
        let end = scrub_preview(&laid, width + 50.0, width, 200.0, (16.0, 9.0)).unwrap();
        assert_eq!(end.picture.x + end.picture.width, width - PREVIEW_EDGE);
        assert_eq!((end.at_seconds, end.moment_seconds), (200.0, 198.0));
    }

    #[test]
    fn the_scrub_preview_shows_moments_a_hundredth_apart() {
        use super::video_controls::{layout, scrub_preview};
        let laid = layout(1216.0, 684.0, false);
        let x = |share: f64| laid.track.x + laid.track.width * share;
        let moment = |share: f64| {
            scrub_preview(&laid, x(share), 1216.0, 50.0, (16.0, 9.0))
                .unwrap()
                .moment_seconds
        };
        assert_eq!(moment(0.004), 0.0);
        assert_eq!(moment(0.012), 0.5);
        assert_eq!(moment(0.019), 0.5);
        assert_eq!(moment(0.5), 25.0);
    }

    #[test]
    fn the_scrub_preview_needs_a_length_a_shape_and_room() {
        use super::video_controls::{layout, scrub_preview};
        let laid = layout(1216.0, 684.0, false);
        let x = laid.track.x + 100.0;
        assert!(scrub_preview(&laid, x, 1216.0, 0.0, (16.0, 9.0)).is_none());
        assert!(scrub_preview(&laid, x, 1216.0, f64::INFINITY, (16.0, 9.0)).is_none());
        assert!(scrub_preview(&laid, x, 1216.0, 10.0, (0.0, 9.0)).is_none());
        // A video too short for a 32 pt thumbnail over its bar gets none; a narrow one shrinks.
        let short = layout(400.0, 60.0, false);
        assert!(scrub_preview(&short, 200.0, 400.0, 10.0, (16.0, 9.0)).is_none());
        let narrow = layout(96.0, 300.0, false);
        let shrunk = scrub_preview(&narrow, 48.0, 96.0, 10.0, (16.0, 9.0)).unwrap();
        assert_eq!((shrunk.picture.x, shrunk.picture.width), (8.0, 80.0));
    }

    #[test]
    fn the_scrub_preview_shows_while_the_pointer_is_on_the_bar_or_drags_it() {
        use super::video_controls::{layout, Controls, Menu, Style};
        let (width, height) = (1216.0, 684.0);
        let laid = layout(width, height, false);
        let on_bar = (laid.track.x + laid.track.width * 0.25, laid.track.y + 2.0);
        let mut controls = Controls::new(Style::Video, 100.0);
        let preview = |controls: &mut Controls, shown: &video_controls::Playback, now: f64| {
            controls
                .video_frame(width, height, shown, 0, at(now), (0.0, 0.0))
                .and_then(|frame| frame.preview)
        };
        // No shape known: no preview.
        controls.pointer_moved(width, height, on_bar.0, on_bar.1, true, at(100.0));
        assert!(preview(&mut controls, &playback(), 100.0).is_none());
        controls.set_shape(1920.0, 1080.0);
        let shown = preview(&mut controls, &playback(), 100.0).expect("on the bar");
        assert_eq!(shown.at_seconds, 1.0);
        // A resting page player shows it too; an open menu hides it.
        let resting = video_controls::Playback {
            resting: true,
            ..playback()
        };
        assert!(preview(&mut controls, &resting, 100.0).is_some());
        let menu = video_controls::Playback {
            menu: Menu::Main { focus: 0 },
            ..playback()
        };
        assert!(preview(&mut controls, &menu, 100.0).is_none());
        // Over the picture, none; off the controls, none.
        controls.pointer_moved(width, height, on_bar.0, 200.0, true, at(100.1));
        assert!(preview(&mut controls, &playback(), 100.1).is_none());
        // A drag carries it, wherever the pointer goes.
        controls.pointer_moved(width, height, on_bar.0, on_bar.1, true, at(100.2));
        controls.press(
            width,
            height,
            on_bar.0,
            on_bar.1,
            &playback(),
            at(100.2),
            0.0,
        );
        controls.drag(laid.track.x + laid.track.width * 0.75);
        controls.pointer_left(at(100.3));
        controls.pointer_moved(
            width,
            height,
            laid.track.x + laid.track.width * 0.75,
            on_bar.1 - 300.0,
            true,
            at(100.3),
        );
        controls.drag(laid.track.x + laid.track.width * 0.75);
        let dragged = preview(&mut controls, &playback(), 100.3).expect("dragging");
        assert_eq!(dragged.at_seconds, 3.0);
        controls.release();
        assert!(preview(&mut controls, &playback(), 100.4).is_none());
    }

    #[test]
    fn a_press_holds_its_bar_until_let_go_and_a_resting_player_wakes_first() {
        use super::video_controls::{layout, Controls, Drag, Hold, Press, Style};
        use crate::glyph::MediaGlyph;
        let (width, height) = (1216.0, 684.0);
        let laid = layout(width, height, true);
        let mut controls = Controls::new(Style::Video, 100.0);
        let resting = video_controls::Playback {
            resting: true,
            ..playback()
        };
        assert_eq!(
            controls.press(width, height, 600.0, laid.track.y, &resting, at(100.0), 0.0),
            Some(Press::Wake)
        );
        assert_eq!(
            controls.press(
                width,
                height,
                600.0,
                laid.track.y,
                &playback(),
                at(100.0),
                0.0
            ),
            Some(Press::Scrub(laid.track))
        );
        assert_eq!(
            controls.drag(laid.track.x + laid.track.width * 0.75),
            Some(Drag::Seek(0.75))
        );
        assert_eq!(controls.drag(-50.0), Some(Drag::Seek(0.0)));
        // While the bar is held, the controls stay and the knob shows, wherever the pointer is.
        let playing = video_controls::Playback {
            playing: true,
            ..playback()
        };
        assert!(controls
            .video_frame(width, height, &playing, 0, at(150.0), (0.0, 0.0))
            .is_some_and(|frame| frame.knob && frame.panel == 1.0));
        assert_eq!(controls.release(), Some(Hold::Scrub(laid.track)));
        assert_eq!(controls.drag(10.0), None);
        // The volume slider, once open.
        controls.pointer_moved(
            width,
            height,
            laid.mute.x + 24.0,
            laid.mute.y + 24.0,
            true,
            at(101.0),
        );
        let press = controls.press(
            width,
            height,
            laid.volume_bar.x + 26.0,
            laid.volume_bar.y,
            &playback(),
            at(101.0),
            0.0,
        );
        assert_eq!(press, Some(Press::Volume(laid.volume_bar)));
        assert_eq!(
            controls.drag(laid.volume_bar.x + 13.0),
            Some(Drag::Volume(0.25))
        );
        controls.release();
        // The audio pill: its play button, and nothing between the bar and the speaker.
        let mut pill = Controls::new(Style::Sound, 100.0);
        assert_eq!(
            pill.press(300.0, 54.0, 20.0, 27.0, &playback(), at(100.0), 70.0),
            Some(Press::Play)
        );
        assert_eq!(
            pill.press(300.0, 54.0, 218.0, 27.0, &playback(), at(100.0), 70.0),
            None
        );
        assert!(
            pill.operates(300.0, 54.0, 218.0, 27.0, &playback())
                && !pill.operates(300.0, 54.0, 301.0, 27.0, &playback())
        );
        let frame = pill.sound_frame(300.0, 54.0, &playback(), 70.0);
        assert_eq!(
            (frame.played, frame.knob, frame.mute),
            (0.25, false, Some(MediaGlyph::PillSound))
        );
        assert_eq!(
            (frame.play, frame.more),
            (MediaGlyph::PillPlay, MediaGlyph::PillMore)
        );
        let muted = video_controls::Playback {
            playing: true,
            muted: true,
            ..playback()
        };
        let frame = pill.sound_frame(300.0, 54.0, &muted, 70.0);
        assert_eq!(
            (frame.play, frame.mute),
            (MediaGlyph::PillPause, Some(MediaGlyph::PillMuted))
        );
        let silent = pill.sound_frame(
            300.0,
            54.0,
            &video_controls::Playback {
                volume: 0.0,
                ..playback()
            },
            70.0,
        );
        assert_eq!(
            silent.mute,
            Some(MediaGlyph::PillMuted),
            "no volume crosses the speaker out too"
        );
        let without = pill.sound_frame(
            300.0,
            54.0,
            &video_controls::Playback {
                has_audio: false,
                ..playback()
            },
            70.0,
        );
        assert_eq!(without.mute, None);
        pill.pointer_moved(300.0, 54.0, 20.0, 27.0, true, at(100.0));
        assert!(pill.sound_frame(300.0, 54.0, &playback(), 70.0).knob);
    }

    #[test]
    fn keys_go_to_the_menu_first_and_wake_a_resting_player() {
        use super::video_controls::{Controls, KeyRoute, Menu, Style};
        let controls = Controls::new(Style::Video, 100.0);
        let resting = video_controls::Playback {
            resting: true,
            ..playback()
        };
        assert_eq!(
            controls.route_key(Some(Key::Space), true, &resting),
            KeyRoute::Wake
        );
        assert_eq!(
            controls.route_key(Some(Key::Escape), true, &resting),
            KeyRoute::Pass
        );
        assert_eq!(
            controls.route_key(Some(Key::Space), false, &resting),
            KeyRoute::Pass
        );
        assert_eq!(controls.route_key(None, true, &playback()), KeyRoute::Pass);
        assert_eq!(
            controls.route_key(Some(Key::Right), true, &playback()),
            KeyRoute::Player(Key::Right)
        );
        let menu = video_controls::Playback {
            menu: Menu::Main { focus: 0 },
            ..playback()
        };
        assert_eq!(
            controls.route_key(Some(Key::Left), true, &menu),
            KeyRoute::Menu(Key::Left)
        );
        // The audio pill has no menu.
        assert_eq!(
            Controls::new(Style::Sound, 100.0).route_key(Some(Key::Left), true, &menu),
            KeyRoute::Player(Key::Left)
        );
        // The wheel scrolls only the open speed list, over its sheet.
        let speeds = video_controls::Playback {
            menu: Menu::speeds(1.0, 1216.0, 684.0),
            ..playback()
        };
        assert!(controls.wheel_scrolls(1216.0, 684.0, 1100.0, 500.0, &speeds));
        assert!(!controls.wheel_scrolls(1216.0, 684.0, 100.0, 500.0, &speeds));
        assert!(!controls.wheel_scrolls(1216.0, 684.0, 1100.0, 500.0, &menu));
    }

    #[test]
    fn boxes_hold_points_as_appkit_counts_them() {
        use super::video_controls::Rect;
        let rect = Rect {
            x: 10.0,
            y: 10.0,
            width: 20.0,
            height: 10.0,
        };
        assert!(
            rect.contains(10.0, 10.0) && !rect.contains(30.0, 15.0) && !rect.contains(15.0, 20.0)
        );
        assert!(!Rect::default().contains(0.0, 0.0));
        assert_eq!(
            rect.inset(-4.0, 2.0),
            Rect {
                x: 6.0,
                y: 12.0,
                width: 28.0,
                height: 6.0
            }
        );
        assert_eq!(
            rect.union(Rect {
                x: 0.0,
                y: 15.0,
                width: 5.0,
                height: 10.0
            }),
            Rect {
                x: 0.0,
                y: 10.0,
                width: 30.0,
                height: 15.0
            }
        );
    }
}
