//! Layout and pointer handling for the colour picker a swatch opens (256 pt wide): the
//! saturation/brightness square, eyedropper, sample, hue bar, HEX or RGB fields and presets.
//! The colour model lives in `appearance::picker`. Shells draw backgrounds, gradients and knobs
//! at these frames and place the native text fields where they say.

use crate::board::card::Font;
use crate::dialog::layout::{Line, Measure};
use crate::geometry::Rect;
use crate::settings::appearance::picker::{hsv, Picker, CHANNELS_MODE, HEX_MODE, PRESETS};
use crate::theme::Token;

pub const WIDTH: f64 = 256.0;
pub const HEIGHT: f64 = 447.0;
const SQUARE: Rect = Rect {
    x: 13.0,
    y: 13.0,
    width: 230.0,
    height: 160.0,
};
const DROPPER: Rect = Rect {
    x: 13.0,
    y: 185.0,
    width: 24.0,
    height: 24.0,
};
/// Logged by shells whose platform has no eyedropper (Windows and Linux). Pressing the
/// eyedropper there does nothing.
pub const EYEDROPPER_NOT_HERE: &str = "the eyedropper is not on this platform yet";

/// Inset of the eyedropper glyph within its button, in pt.
const DROPPER_GLYPH_INSET: f64 = 5.0;
const SAMPLE: Rect = Rect {
    x: 45.0,
    y: 185.0,
    width: 24.0,
    height: 24.0,
};
const HUE_BAR: Rect = Rect {
    x: 77.0,
    y: 191.0,
    width: 166.0,
    height: 12.0,
};
/// A press within 4 pt above or below the hue bar counts as on it.
const HUE_REACH: f64 = 4.0;
const MODE: Rect = Rect {
    x: 13.0,
    y: 221.0,
    width: 39.5,
    height: 28.0,
};
const INPUT: Rect = Rect {
    x: 58.5,
    y: 221.0,
    width: 184.5,
    height: 28.0,
};
/// The native hex field, inside `INPUT`.
const HEX_FIELD: Rect = Rect {
    x: 62.0,
    y: 227.0,
    width: 177.0,
    height: 16.0,
};
/// The three channel backgrounds split `INPUT`'s width, 4 pt apart, starting 6 pt after the
/// mode button. Each field sits 4 pt in and 6 pt down, 16 pt tall.
const CHANNEL_GAP: f64 = 4.0;
const CHANNELS_AFTER_MODE: f64 = 6.0;
const CHANNEL_FIELD_INSET: f64 = 4.0;
const CHANNEL_FIELD_DOWN: f64 = 6.0;
const FIELD_LINE: f64 = 16.0;
/// "HEX" or "RGB": 10 px semibold, 0.25 pt tracking, centred on the button 7 pt down on a
/// 14 px line.
const MODE_FONT: Font = Font::sans(10.0, Font::SEMIBOLD);
const MODE_TRACKING: f64 = 0.25;
const MODE_WORDS_DOWN: f64 = 7.0;
const MODE_LINE: f64 = 14.0;
const MODE_WORDS_MOST: f64 = 40.0;
const RULE: Rect = Rect {
    x: 13.0,
    y: 261.0,
    width: 230.0,
    height: 1.0,
};
/// Presets: eight columns across 230 pt, each swatch 20 pt at the left of its column, rows
/// 24 pt apart. The chosen and hovered swatches grow 1 pt on each side.
const PRESETS_LEFT: f64 = 13.0;
const PRESETS_TOP: f64 = 270.0;
const PRESETS_ACROSS: usize = 8;
const PRESET_PITCH: f64 = 29.25;
const PRESET_ROW: f64 = 24.0;
const PRESET: f64 = 20.0;
const PRESET_GROWN: f64 = 1.0;
const SQUARE_KNOB: f64 = 14.0;
const HUE_KNOB: f64 = 12.0;

/// Hue bar gradient stops from red back to red, as (position, RGB).
pub const HUE_STOPS: [(f64, [u8; 3]); 7] = [
    (0.0, [255, 0, 0]),
    (0.17, [255, 255, 0]),
    (0.33, [0, 255, 0]),
    (0.5, [0, 255, 255]),
    (0.67, [0, 0, 255]),
    (0.83, [255, 0, 255]),
    (1.0, [255, 0, 0]),
];

/// Formats a swatch label: `#` plus six upper-case hex digits, with or without a leading `#`
/// in `hex`.
pub fn swatch_hex(hex: &str) -> String {
    format!("#{}", hex.trim_start_matches('#').to_uppercase())
}

/// Hit test that excludes the far edges, as platform views do.
fn holds(rect: &Rect, x: f64, y: f64) -> bool {
    x >= rect.x && x < rect.max_x() && y >= rect.y && y < rect.max_y()
}

fn preset_frame(index: usize) -> Rect {
    let (column, row) = (index % PRESETS_ACROSS, index / PRESETS_ACROSS);
    Rect::new(
        PRESETS_LEFT + column as f64 * PRESET_PITCH,
        PRESETS_TOP + row as f64 * PRESET_ROW,
        PRESET,
        PRESET,
    )
}

fn channel_ground(index: usize) -> Rect {
    let left = MODE.max_x() + CHANNELS_AFTER_MODE;
    let width = (INPUT.max_x() - left - 2.0 * CHANNEL_GAP) / 3.0;
    Rect::new(
        left + index as f64 * (width + CHANNEL_GAP),
        INPUT.y,
        width,
        INPUT.height,
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drag {
    Square,
    Hue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Nothing,
    /// The colour changed via the square, the hue bar (a drag follows) or a preset.
    Changed,
    /// The shell should run the native eyedropper and report back via `PickerView::sampled`.
    Eyedropper,
    /// The fields switched between hex and RGB channels.
    ModeTurned,
}

/// An open picker: its colour plus drag, hover and field-mode state.
#[derive(Clone, Debug, PartialEq)]
pub struct PickerView {
    pub picker: Picker,
    /// Show RGB channel fields instead of hex.
    channels: bool,
    dragging: Option<Drag>,
    hovered_preset: Option<usize>,
    hovered_mode: bool,
}

/// A round handle, `side` pt across, centred on a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Knob {
    pub frame: Rect,
}

impl Knob {
    fn at(x: f64, y: f64, side: f64) -> Self {
        Self {
            frame: Rect::new(x - side / 2.0, y - side / 2.0, side, side),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub at: f64,
    pub channels: [u8; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct PresetPlace {
    /// Six upper-case hex digits.
    pub hex: String,
    pub frame: Rect,
    /// The current colour, drawn with a ring.
    pub chosen: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub width: f64,
    pub height: f64,
    pub square: Rect,
    pub square_knob: Knob,
    pub dropper: Rect,
    pub dropper_glyph: Rect,
    pub sample: Rect,
    pub hue_bar: Rect,
    pub hue_stops: Vec<Stop>,
    pub hue_knob: Knob,
    pub mode: Rect,
    pub mode_hovered: bool,
    pub mode_words: Line,
    /// Show RGB channel fields instead of hex.
    pub channels: bool,
    /// Field backgrounds: one for hex, or three for the channels.
    pub grounds: Vec<Rect>,
    /// Frames for the native hex field and the three channel fields.
    pub hex_field: Rect,
    pub channel_fields: Vec<Rect>,
    pub rule: Rect,
    pub presets: Vec<PresetPlace>,
}

impl PickerView {
    /// Opens on `hex` in hex mode.
    pub fn open(hex: &str) -> Self {
        Self {
            picker: Picker::open(hex),
            channels: false,
            dragging: None,
            hovered_preset: None,
            hovered_mode: false,
        }
    }

    pub fn layout(&self, measure: &mut Measure) -> Layout {
        let shown = self.picker.hex();
        let (_, saturation, brightness) = hsv(shown);
        let name = if self.channels {
            CHANNELS_MODE
        } else {
            HEX_MODE
        };
        let wide = measure(name, MODE_FONT) + MODE_TRACKING * name.chars().count() as f64;
        Layout {
            width: WIDTH,
            height: HEIGHT,
            square: SQUARE,
            square_knob: Knob::at(
                SQUARE.x + saturation * SQUARE.width,
                SQUARE.y + (1.0 - brightness) * SQUARE.height,
                SQUARE_KNOB,
            ),
            dropper: DROPPER,
            dropper_glyph: DROPPER.outset(-DROPPER_GLYPH_INSET),
            sample: SAMPLE,
            hue_bar: HUE_BAR,
            hue_stops: HUE_STOPS
                .iter()
                .map(|&(at, channels)| Stop { at, channels })
                .collect(),
            hue_knob: Knob::at(
                HUE_BAR.x + self.picker.hue() / 360.0 * HUE_BAR.width,
                HUE_BAR.mid_y(),
                HUE_KNOB,
            ),
            mode: MODE,
            mode_hovered: self.hovered_mode,
            mode_words: Line::new(
                name,
                MODE.mid_x() - wide / 2.0,
                MODE.y + MODE_WORDS_DOWN,
                MODE_WORDS_MOST,
                MODE_LINE,
                MODE_FONT,
                Token::MutedForeground,
            ),
            channels: self.channels,
            grounds: if self.channels {
                (0..3).map(channel_ground).collect()
            } else {
                vec![INPUT]
            },
            hex_field: HEX_FIELD,
            channel_fields: (0..3)
                .map(|index| {
                    let ground = channel_ground(index);
                    Rect::new(
                        ground.x + CHANNEL_FIELD_INSET,
                        ground.y + CHANNEL_FIELD_DOWN,
                        ground.width - 2.0 * CHANNEL_FIELD_INSET,
                        FIELD_LINE,
                    )
                })
                .collect(),
            rule: RULE,
            presets: PRESETS
                .iter()
                .enumerate()
                .map(|(index, &preset)| {
                    let chosen = preset == shown;
                    let frame = preset_frame(index);
                    let grown = chosen || self.hovered_preset == Some(index);
                    PresetPlace {
                        hex: preset.to_owned(),
                        frame: if grown {
                            frame.outset(PRESET_GROWN)
                        } else {
                            frame
                        },
                        chosen,
                    }
                })
                .collect(),
        }
    }

    fn preset_at(x: f64, y: f64) -> Option<usize> {
        (0..PRESETS.len()).find(|&index| holds(&preset_frame(index), x, y))
    }

    pub fn press(&mut self, x: f64, y: f64) -> Press {
        let reach = Rect::new(
            HUE_BAR.x,
            HUE_BAR.y - HUE_REACH,
            HUE_BAR.width,
            HUE_BAR.height + 2.0 * HUE_REACH,
        );
        if holds(&SQUARE, x, y) {
            self.dragging = Some(Drag::Square);
        } else if holds(&reach, x, y) {
            self.dragging = Some(Drag::Hue);
        } else if holds(&DROPPER, x, y) {
            return Press::Eyedropper;
        } else if let Some(index) = Self::preset_at(x, y) {
            self.picker.set(PRESETS[index]);
            return Press::Changed;
        } else if holds(&MODE, x, y) {
            self.channels = !self.channels;
            return Press::ModeTurned;
        }
        if self.drag(x, y) {
            Press::Changed
        } else {
            Press::Nothing
        }
    }

    /// Moves the colour along the square or hue bar the press started on. Returns whether a
    /// drag was in progress.
    pub fn drag(&mut self, x: f64, y: f64) -> bool {
        match self.dragging {
            Some(Drag::Square) => self.picker.drag_square(
                (x - SQUARE.x) / SQUARE.width,
                (y - SQUARE.y) / SQUARE.height,
            ),
            Some(Drag::Hue) => self.picker.drag_hue((x - HUE_BAR.x) / HUE_BAR.width),
            None => return false,
        }
        true
    }

    pub fn release(&mut self) {
        self.dragging = None;
    }

    /// Updates hover for a pointer at `at`, or `None` once it leaves. Returns whether the
    /// picker needs redrawing.
    pub fn hover(&mut self, at: Option<(f64, f64)>) -> bool {
        let (preset, mode) = at.map_or((None, false), |(x, y)| {
            (Self::preset_at(x, y), holds(&MODE, x, y))
        });
        let changed = (preset, mode) != (self.hovered_preset, self.hovered_mode);
        (self.hovered_preset, self.hovered_mode) = (preset, mode);
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::typeset::test_measure::sevens;

    #[test]
    fn the_picker_places_its_parts_and_rings_the_preset_it_shows() {
        let view = PickerView::open("#ff0000");
        let laid = view.layout(&mut sevens);
        assert_eq!((laid.width, laid.height), (256.0, 447.0));
        // Pure red: the square knob at top right, the hue knob at the bar's start.
        assert_eq!(laid.square_knob.frame, Rect::new(236.0, 6.0, 14.0, 14.0));
        assert_eq!(laid.hue_knob.frame, Rect::new(71.0, 191.0, 12.0, 12.0));
        assert_eq!(laid.dropper_glyph, Rect::new(18.0, 190.0, 14.0, 14.0));
        // "HEX" is 21 pt wide plus 0.75 pt of tracking, centred on the button.
        assert_eq!(
            laid.mode_words.frame,
            Rect::new(32.75 - 10.875, 228.0, 40.0, 14.0)
        );
        assert_eq!((laid.channels, laid.grounds.clone()), (false, vec![INPUT]));
        assert_eq!(
            laid.channel_fields[0],
            Rect::new(62.5, 227.0, 176.5 / 3.0 - 8.0, 16.0)
        );
        assert_eq!(laid.presets.len(), 56);
        assert_eq!(
            laid.presets[9].frame,
            Rect::new(13.0 + 29.25, 294.0, 20.0, 20.0)
        );
        let red = laid
            .presets
            .iter()
            .position(|preset| preset.hex == "F44336");
        assert!(red.is_some_and(|red| !laid.presets[red].chosen));
        let laid = PickerView::open("F44336").layout(&mut sevens);
        assert_eq!(
            laid.presets[24].frame,
            Rect::new(12.0, 341.0, 22.0, 22.0),
            "the chosen one is a point larger"
        );
        assert!(laid.presets[24].chosen);
        assert_eq!(
            laid.hue_stops.first().map(|stop| stop.channels),
            Some([255, 0, 0])
        );
    }

    #[test]
    fn a_press_drags_the_square_or_the_bar_takes_a_preset_or_turns_the_mode() {
        let mut view = PickerView::open("FF0000");
        // A drag that starts in the square clamps at its edges: bottom left is black, top right
        // is the hue.
        assert_eq!(view.press(13.0, 100.0), Press::Changed);
        assert!(view.drag(13.0, 400.0));
        assert_eq!(view.picker.hex(), "000000");
        assert!(view.drag(400.0, 13.0));
        assert_eq!(view.picker.hex(), "FF0000");
        view.release();
        assert!(!view.drag(100.0, 100.0), "no drag once the button is up");
        // 4 pt below the bar still hits it; halfway along is cyan.
        assert_eq!(view.press(160.0, 206.0), Press::Changed);
        assert_eq!(view.picker.hex(), "00FFFF");
        view.release();
        assert_eq!(view.press(20.0, 190.0), Press::Eyedropper);
        assert_eq!(view.press(13.0 + 29.25, 270.0), Press::Changed);
        assert_eq!(view.picker.hex(), "E0E0E0");
        assert_eq!(view.press(14.0, 222.0), Press::ModeTurned);
        let laid = view.layout(&mut sevens);
        assert_eq!(
            (
                laid.channels,
                laid.grounds.len(),
                laid.mode_words.text.as_str()
            ),
            (true, 3, "RGB")
        );
        assert_eq!(view.press(250.0, 440.0), Press::Nothing);
    }

    #[test]
    fn a_swatch_writes_its_colour_with_a_hash_in_upper_case() {
        assert_eq!(
            (swatch_hex("#e34a49"), swatch_hex("E34A49")),
            ("#E34A49".into(), "#E34A49".into())
        );
    }

    #[test]
    fn the_pointer_grows_the_preset_it_is_over_and_lights_the_mode_button() {
        let mut view = PickerView::open("123456");
        assert!(view.hover(Some((14.0, 271.0))));
        assert!(!view.hover(Some((15.0, 272.0))), "still over the same one");
        assert_eq!(
            view.layout(&mut sevens).presets[0].frame,
            Rect::new(12.0, 269.0, 22.0, 22.0)
        );
        assert!(view.hover(Some((20.0, 230.0))));
        assert!(view.layout(&mut sevens).mode_hovered);
        assert!(view.hover(None));
        assert!(!view.layout(&mut sevens).mode_hovered);
    }
}
