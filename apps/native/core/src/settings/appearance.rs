//! Appearance settings: whether the light or dark theme applies, and each theme's colours and
//! contrast, edited independently. Saved in the settings file and fed to the theme kernel.

use crate::settings::page::{Control, Page, Row, Section};
use crate::strings::{self, key, Key};
use crate::theme::{Appearance, Mode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which theme applies: light, dark, or follow the system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Choice {
    Light,
    Dark,
    System,
}

/// Id stored for a preset or accent that was hand-edited away from every named one.
pub const CUSTOM: &str = "custom";
/// Id of the accent that uses the theme's text colour.
pub const NEUTRAL: &str = "neutral";

/// Accents in dropdown order: id, name key and colour. `neutral` has no fixed colour; it uses
/// the theme's text colour.
pub const ACCENTS: [(&str, Key, Option<&str>); 9] = [
    (NEUTRAL, key::APPEARANCE_ACCENT_NEUTRAL, None),
    ("violet", key::APPEARANCE_ACCENT_VIOLET, Some("#4382F2")),
    ("blue", key::APPEARANCE_ACCENT_BLUE, Some("#0090D4")),
    ("teal", key::APPEARANCE_ACCENT_TEAL, Some("#009B9B")),
    ("green", key::APPEARANCE_ACCENT_GREEN, Some("#30A054")),
    ("amber", key::APPEARANCE_ACCENT_AMBER, Some("#AA8000")),
    ("orange", key::APPEARANCE_ACCENT_ORANGE, Some("#CC6900")),
    ("red", key::APPEARANCE_ACCENT_RED, Some("#E34A49")),
    ("pink", key::APPEARANCE_ACCENT_PINK, Some("#D64B97")),
];

/// A preset's values for one mode: background, foreground, accent, contrast.
type PresetColors = (&'static str, &'static str, &'static str, f64);

/// Presets in dropdown order: id, name key, light values, dark values. Those after Terminal
/// are the published palettes of the themes they are named for.
pub const PRESETS: [(&str, Key, PresetColors, PresetColors); 6] = [
    (
        "northroom",
        key::APPEARANCE_PRESET_NORTHROOM,
        ("#FAFAFB", "#111113", "#111113", 50.0),
        ("#111111", "#EEEEF0", "#EEEEF0", 50.0),
    ),
    (
        "terminal",
        key::APPEARANCE_PRESET_TERMINAL,
        ("#F1F9F3", "#101911", "#30A054", 50.0),
        ("#0B110D", "#DFECE1", "#30A054", 50.0),
    ),
    (
        "nord",
        key::APPEARANCE_PRESET_NORD,
        ("#ECEFF4", "#2E3440", "#5E81AC", 50.0),
        ("#2E3440", "#D8DEE9", "#88C0D0", 50.0),
    ),
    (
        "rose-pine",
        key::APPEARANCE_PRESET_ROSE_PINE,
        ("#FAF4ED", "#575279", "#907AA9", 50.0),
        ("#191724", "#E0DEF4", "#C4A7E7", 50.0),
    ),
    (
        "gruvbox",
        key::APPEARANCE_PRESET_GRUVBOX,
        ("#FBF1C7", "#3C3836", "#D65D0E", 50.0),
        ("#282828", "#EBDBB2", "#FE8019", 50.0),
    ),
    (
        "solarized",
        key::APPEARANCE_PRESET_SOLARIZED,
        ("#FDF6E3", "#586E75", "#268BD2", 50.0),
        ("#002B36", "#93A1A1", "#268BD2", 50.0),
    ),
];

/// An accent's colour in a theme whose text colour is `foreground`.
pub fn accent_color(accent: &str, foreground: &str) -> Option<String> {
    ACCENTS
        .iter()
        .find(|(id, _, _)| *id == accent)
        .map(|(_, _, color)| color.unwrap_or(foreground).to_owned())
}

/// The named accent matching `color`, so a typed hex still shows its name, or `custom`.
pub fn accent_for_color(color: &str, foreground: &str) -> &'static str {
    ACCENTS
        .iter()
        .find(|(_, _, fixed)| fixed.unwrap_or(foreground).eq_ignore_ascii_case(color))
        .map_or(CUSTOM, |(id, _, _)| id)
}

/// One mode's theme settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeChoice {
    pub preset: String,
    pub accent_preset: String,
    pub accent_color: String,
    pub cta_color: String,
    pub background: String,
    pub foreground: String,
    pub contrast: f64,
}

impl ThemeChoice {
    pub fn shipped(mode: Mode) -> Self {
        let mut theme = Self {
            preset: String::new(),
            accent_preset: String::new(),
            accent_color: String::new(),
            cta_color: String::new(),
            background: String::new(),
            foreground: String::new(),
            contrast: 50.0,
        };
        theme.apply_preset("northroom", mode);
        theme
    }

    /// Applies a preset's colours and name. Every shipped preset uses a monochrome call to
    /// action in its text colour.
    pub fn apply_preset(&mut self, preset: &str, mode: Mode) -> bool {
        let Some((id, _, light, dark)) = PRESETS.iter().find(|(id, ..)| *id == preset) else {
            return false;
        };
        let (background, foreground, accent, contrast) =
            if mode == Mode::Light { *light } else { *dark };
        self.background = background.to_owned();
        self.foreground = foreground.to_owned();
        self.accent_color = accent.to_owned();
        self.cta_color = foreground.to_owned();
        self.contrast = contrast;
        self.preset = (*id).to_owned();
        self.accent_preset = accent_for_color(accent, foreground).to_owned();
        true
    }

    /// Marks the theme custom after a manual edit.
    fn mark_custom(&mut self) {
        self.preset = CUSTOM.to_owned();
    }

    /// Picks a named accent and marks the theme custom.
    pub fn set_accent(&mut self, accent: &str) -> bool {
        let Some(color) = accent_color(accent, &self.foreground) else {
            return false;
        };
        self.accent_color = color;
        self.accent_preset = accent.to_owned();
        self.mark_custom();
        true
    }

    pub fn set_accent_color(&mut self, color: &str) {
        self.accent_preset = accent_for_color(color, &self.foreground).to_owned();
        self.accent_color = color.to_owned();
        self.mark_custom();
    }

    pub fn set_cta_color(&mut self, color: &str) {
        self.cta_color = color.to_owned();
        self.mark_custom();
    }

    pub fn set_background(&mut self, color: &str) {
        self.background = color.to_owned();
        self.mark_custom();
    }

    /// A monochrome accent or call to action follows the text colour.
    pub fn set_foreground(&mut self, color: &str) {
        if accent_for_color(&self.accent_color, &self.foreground) == NEUTRAL {
            self.accent_color = color.to_owned();
        }
        if self.cta_color.eq_ignore_ascii_case(&self.foreground) {
            self.cta_color = color.to_owned();
        }
        self.foreground = color.to_owned();
        self.mark_custom();
    }

    pub fn set_contrast(&mut self, contrast: f64) {
        self.contrast = contrast.clamp(0.0, 100.0).round();
        self.mark_custom();
    }

    /// The input the theme kernel derives every colour from.
    pub fn appearance(&self) -> Appearance {
        Appearance {
            background: self.background.clone(),
            foreground: self.foreground.clone(),
            accent_color: self.accent_color.clone(),
            cta_color: self.cta_color.clone(),
            contrast: self.contrast,
        }
    }
}

/// The settings file's `appearance` section. Missing fields take their defaults, so a person
/// or agent only writes what they change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: Choice,
    pub light: ThemeChoice,
    pub dark: ThemeChoice,
    #[serde(skip)]
    path: Option<PathBuf>,
    /// A check's `NR_THEME`, shown as selected in Settings until the user picks a theme, so
    /// the check's reference image shows it selected.
    #[serde(skip)]
    check_choice: Option<Choice>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Choice::System,
            light: ThemeChoice::shipped(Mode::Light),
            dark: ThemeChoice::shipped(Mode::Dark),
            path: None,
            check_choice: None,
        }
    }
}

impl Settings {
    /// Loads the appearance section of `file` (`settings::file::APPEARANCE`), falling back to
    /// defaults if it is missing or invalid.
    pub fn open(file: &Path) -> Self {
        Self::read(file).unwrap_or_else(|_| Self {
            path: Some(file.to_path_buf()),
            ..Self::default()
        })
    }

    /// Like `open`, but fails if `file` is not a valid settings file at all (say, a bad hand
    /// edit), so the caller can keep its current choices.
    pub fn read(file: &Path) -> Result<Self, String> {
        let kept = crate::settings::file::read(file)?
            .remove(crate::settings::file::APPEARANCE)
            .and_then(|kept| serde_json::from_value::<Self>(kept).ok());
        Ok(Self {
            path: Some(file.to_path_buf()),
            ..kept.unwrap_or_default()
        })
    }

    /// Shows a check's theme (`dark` or light) as selected; `None` shows the saved one.
    pub fn check_choice(&mut self, dark: Option<bool>) {
        self.check_choice = dark.map(|dark| if dark { Choice::Dark } else { Choice::Light });
    }

    /// The theme Settings shows as selected.
    pub fn shown_choice(&self) -> Choice {
        self.check_choice.unwrap_or(self.theme)
    }

    /// Sets the theme and drops any check override.
    pub fn choose(&mut self, choice: Choice) {
        self.theme = choice;
        self.check_choice = None;
    }

    pub fn theme_mut(&mut self, mode: Mode) -> &mut ThemeChoice {
        if mode == Mode::Light {
            &mut self.light
        } else {
            &mut self.dark
        }
    }

    pub fn theme_of(&self, mode: Mode) -> &ThemeChoice {
        if mode == Mode::Light {
            &self.light
        } else {
            &self.dark
        }
    }

    /// The active mode, given the system's appearance.
    pub fn mode(&self, system_is_dark: bool) -> Mode {
        match self.theme {
            Choice::Light => Mode::Light,
            Choice::Dark => Mode::Dark,
            Choice::System if system_is_dark => Mode::Dark,
            Choice::System => Mode::Light,
        }
    }

    /// Writes the choices to the settings file under their own key.
    pub fn save(&self) -> std::io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let value = serde_json::to_value(self).map_err(std::io::Error::other)?;
        crate::settings::file::write(path, crate::settings::file::APPEARANCE, value)
    }
}

/// Action ids for the Appearance page's controls (`Settings::act`). Per-mode controls are
/// `appearance.<light|dark>.<preset|accent|accent-color|cta|background|foreground|contrast>`.
pub const THEME_ACTION: &str = "appearance.theme";
/// The preset and accent dropdowns are narrower than the page default (`SELECT_WIDTH`).
const PRESET_SELECT_WIDTH: f64 = 160.0;
const ACCENT_SELECT_WIDTH: f64 = 128.0;
/// Options of the theme control, in order.
const CHOICES: [(Choice, Key); 3] = [
    (Choice::Light, key::ACCOUNT_THEME_LIGHT),
    (Choice::Dark, key::ACCOUNT_THEME_DARK),
    (Choice::System, key::ACCOUNT_THEME_SYSTEM),
];

/// Whether `value` is a valid theme colour: `#` and six hex digits.
fn well_formed(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].chars().all(|c| c.is_ascii_hexdigit())
}

impl Settings {
    /// The Appearance tab's page in `language`: which theme applies, then each mode's preset,
    /// colours and contrast.
    pub fn page(&self, language: &str) -> Page {
        let say = |key: Key| strings::text(language, key);
        let mut page = Page::titled(say(key::APPEARANCE_PAGE_TITLE));
        page.summary = Some(say(key::APPEARANCE_PAGE_DESCRIPTION).to_owned());
        let mut theme = Section::new(say(key::ACCOUNT_THEME_LABEL));
        let shown = self.shown_choice();
        theme.rows.push(
            Row::new(say(key::ACCOUNT_THEME_LABEL)).with(Control::Theme {
                action: THEME_ACTION.to_owned(),
                labels: CHOICES
                    .iter()
                    .map(|(_, name)| say(*name).to_owned())
                    .collect(),
                selected: CHOICES
                    .iter()
                    .position(|(choice, _)| *choice == shown)
                    .expect("every choice is offered"),
            }),
        );
        page.sections.push(theme);
        page.sections.push(self.mode_section(
            Mode::Light,
            say(key::APPEARANCE_LIGHT_THEME),
            language,
        ));
        page.sections.push(self.mode_section(
            Mode::Dark,
            say(key::APPEARANCE_DARK_THEME),
            language,
        ));
        page
    }

    /// One mode's section: its preset's dropdown at the heading, then the accent (a named
    /// one, with the colour each stands for, beside the colour itself), the call to action,
    /// the background, the text and the contrast. A theme that is none of the named presets
    /// or accents shows the custom choice, listed last.
    fn mode_section(&self, mode: Mode, label: &str, language: &str) -> Section {
        let say = |key: Key| strings::text(language, key);
        let theme = self.theme_of(mode);
        let action = |field: &str| {
            format!(
                "appearance.{}.{field}",
                if mode == Mode::Light { "light" } else { "dark" }
            )
        };
        let color = |field: &str, hex: &str| Control::Color {
            action: action(field),
            hex: hex.to_owned(),
        };
        let custom = |key: Key| crate::settings::page::Choice {
            id: CUSTOM.to_owned(),
            name: say(key).to_owned(),
            swatch: None,
        };

        let mut presets: Vec<_> = PRESETS
            .iter()
            .map(|(id, name, ..)| crate::settings::page::Choice {
                id: (*id).to_owned(),
                name: say(*name).to_owned(),
                swatch: None,
            })
            .collect();
        let named_preset = PRESETS.iter().any(|(id, ..)| *id == theme.preset);
        if !named_preset {
            presets.push(custom(key::APPEARANCE_PRESET_CUSTOM));
        }
        let mut accents: Vec<_> = ACCENTS
            .iter()
            .map(|(id, name, fixed)| crate::settings::page::Choice {
                id: (*id).to_owned(),
                name: say(*name).to_owned(),
                swatch: Some(fixed.unwrap_or(&theme.foreground).to_owned()),
            })
            .collect();
        let named_accent = ACCENTS.iter().any(|(id, ..)| *id == theme.accent_preset);
        if !named_accent {
            accents.push(custom(key::APPEARANCE_ACCENT_CUSTOM));
        }
        let mut section = Section::new(label);
        section.action = Some(Control::select(
            &action("preset"),
            true,
            presets,
            if named_preset { &theme.preset } else { CUSTOM },
            PRESET_SELECT_WIDTH,
        ));
        section.rows = vec![
            Row::new(say(key::APPEARANCE_ACCENT))
                .with(Control::select(
                    &action("accent"),
                    true,
                    accents,
                    if named_accent {
                        &theme.accent_preset
                    } else {
                        CUSTOM
                    },
                    ACCENT_SELECT_WIDTH,
                ))
                .with(color("accent-color", &theme.accent_color)),
            Row::new(say(key::APPEARANCE_CTA)).with(color("cta", &theme.cta_color)),
            Row::new(say(key::APPEARANCE_BACKGROUND)).with(color("background", &theme.background)),
            Row::new(say(key::APPEARANCE_FOREGROUND)).with(color("foreground", &theme.foreground)),
            Row::new(say(key::APPEARANCE_CONTRAST)).with(Control::Slider {
                action: action("contrast"),
                value: theme.contrast,
            }),
        ];
        section
    }

    /// A person used a control of the Appearance page: `action` is the control's
    /// (`THEME_ACTION` or a mode's own), `value` what it now holds: the
    /// theme's index as `CHOICES` lists them, a preset's or an accent's id, a `#rrggbb`
    /// colour, a contrast. Returns whether the choices changed; a malformed colour or an id
    /// that is no preset or accent changes nothing.
    pub fn act(&mut self, action: &str, value: &str) -> bool {
        match action {
            THEME_ACTION => self.choose_listed(value),
            _ => self.set_theme_field(action, value),
        }
    }

    /// Chooses the theme at the index `value` names, as `CHOICES` lists them.
    fn choose_listed(&mut self, value: &str) -> bool {
        let Some((choice, _)) = value
            .parse::<usize>()
            .ok()
            .and_then(|index| CHOICES.get(index))
        else {
            return false;
        };
        self.choose(*choice);
        true
    }

    /// A mode's own control, `appearance.<mode>.<field>`, sets that field of the mode's theme.
    fn set_theme_field(&mut self, action: &str, value: &str) -> bool {
        let Some((mode, field)) = action
            .strip_prefix("appearance.")
            .and_then(|rest| rest.split_once('.'))
        else {
            return false;
        };
        let mode = match mode {
            "light" => Mode::Light,
            "dark" => Mode::Dark,
            _ => return false,
        };
        self.theme_mut(mode).set_field(field, value, mode)
    }
}

impl ThemeChoice {
    /// Sets the theme's `field` (a preset, an accent, a colour or the contrast) to `value`,
    /// for `mode`. Whether it changed: a malformed colour or an unknown id changes nothing.
    fn set_field(&mut self, field: &str, value: &str, mode: Mode) -> bool {
        let colour = well_formed(value);
        match field {
            "preset" => self.apply_preset(value, mode),
            "accent" => self.set_accent(value),
            "accent-color" if colour => {
                self.set_accent_color(value);
                true
            }
            "cta" if colour => {
                self.set_cta_color(value);
                true
            }
            "background" if colour => {
                self.set_background(value);
                true
            }
            "foreground" if colour => {
                self.set_foreground(value);
                true
            }
            "contrast" => match value.parse::<f64>() {
                Ok(contrast) if contrast.is_finite() => {
                    self.set_contrast(contrast);
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
}

/// Contrast at `share` (0 to 1) along the slider: 0 to 100 in whole steps.
pub fn contrast_at(share: f64) -> f64 {
    (share.clamp(0.0, 1.0) * 100.0).round()
}

/// The slider's label: the contrast truncated to a whole number.
pub fn contrast_words(value: f64) -> String {
    (value.trunc() as i64).to_string()
}

/// `hex` (six digits) with `channel` (0 red, 1 green, 2 blue) set from text typed in an RGB
/// field. Parses leading spaces, an optional sign and digits, ignores the rest, and clamps to
/// 0 to 255. `None` if no number was typed.
pub fn with_channel(hex: &str, channel: usize, typed: &str) -> Option<String> {
    let digits = hex.trim_start_matches('#');
    if digits.len() != 6 || channel > 2 {
        return None;
    }
    let mut channels = [0u8; 3];
    for (index, value) in channels.iter_mut().enumerate() {
        *value = u8::from_str_radix(digits.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    let text = typed.trim_start();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let count = rest.chars().take_while(char::is_ascii_digit).count();
    if count == 0 {
        return None;
    }
    // More digits than fit a number are far past 255 already.
    let value = rest[..count].parse::<u64>().unwrap_or(u64::MAX);
    channels[channel] = if negative { 0 } else { value.min(255) as u8 };
    Some(format!(
        "{:02X}{:02X}{:02X}",
        channels[0], channels[1], channels[2]
    ))
}

/// Colour model for the swatch picker: saturation/brightness square, hue bar, hex and RGB
/// fields, and presets. A hex round-trips unchanged.
pub mod picker {
    use super::with_channel;

    /// The words on the button that turns the fields between the hex and the three channels.
    pub const HEX_MODE: &str = "HEX";
    pub const CHANNELS_MODE: &str = "RGB";
    /// Gap between the picker and its swatch, and the minimum margin to the window's edges.
    pub const GAP: f64 = 8.0;
    pub const MARGIN: f64 = 12.0;

    /// Preset colours, eight per row.
    pub const PRESETS: [&str; 56] = [
        "FFFFFF", "E0E0E0", "BDBDBD", "9E9E9E", "757575", "616161", "424242", "000000", //
        "FFCDD2", "F8BBD0", "E1BEE7", "D1C4E9", "C5CAE9", "BBDEFB", "B3E5FC", "B2EBF2", //
        "B2DFDB", "C8E6C9", "DCEDC8", "F0F4C3", "FFF9C4", "FFECB3", "FFE0B2", "FFCCBC", //
        "F44336", "E91E63", "9C27B0", "673AB7", "3F51B5", "2196F3", "03A9F4", "00BCD4", //
        "009688", "4CAF50", "8BC34A", "CDDC39", "FFEB3B", "FFC107", "FF9800", "FF5722", //
        "B71C1C", "880E4F", "4A148C", "311B92", "1A237E", "0D47A1", "01579B", "006064", //
        "FF6F61", "FF8A80", "EA80FC", "B388FF", "82B1FF", "80D8FF", "A7FFEB", "CCFF90",
    ];
    /// Maximum hue. 360 wraps to red (0), so the bar stops short of it.
    const LAST_HUE: f64 = 359.999;
    const HEX_DIGITS: usize = 6;

    /// Channels of `hex` (with or without `#`), 0 to 255. Missing or invalid channels are 0.
    pub fn rgb(hex: &str) -> [u8; 3] {
        let digits = hex.replace('#', "");
        let channel = |at: usize| {
            digits
                .get(at..at + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .unwrap_or(0)
        };
        [channel(0), channel(2), channel(4)]
    }

    /// Hue in degrees, saturation and brightness from 0 to 1.
    pub fn hsv(hex: &str) -> (f64, f64, f64) {
        let [r, g, b] = rgb(hex).map(|channel| f64::from(channel) / 255.0);
        let high = r.max(g).max(b);
        let diff = high - r.min(g).min(b);
        let mut hue = 0.0;
        if diff != 0.0 {
            hue = if high == r {
                ((g - b) / diff) % 6.0
            } else if high == g {
                (b - r) / diff + 2.0
            } else {
                (r - g) / diff + 4.0
            };
        }
        hue = (hue * 60.0 + 360.0) % 360.0;
        (
            if hue.is_nan() { 0.0 } else { hue },
            if high == 0.0 { 0.0 } else { diff / high },
            high,
        )
    }

    /// Six upper-case hex digits, no `#`.
    pub fn hex(hue: f64, saturation: f64, brightness: f64) -> String {
        let chroma = brightness * saturation;
        let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let m = brightness - chroma;
        let (r, g, b) = match hue {
            h if (0.0..60.0).contains(&h) => (chroma, x, 0.0),
            h if (60.0..120.0).contains(&h) => (x, chroma, 0.0),
            h if (120.0..180.0).contains(&h) => (0.0, chroma, x),
            h if (180.0..240.0).contains(&h) => (0.0, x, chroma),
            h if (240.0..300.0).contains(&h) => (x, 0.0, chroma),
            h if (300.0..360.0).contains(&h) => (chroma, 0.0, x),
            _ => (0.0, 0.0, 0.0),
        };
        let channel = |value: f64| ((value + m) * 255.0).round().clamp(0.0, 255.0) as u8;
        format!("{:02X}{:02X}{:02X}", channel(r), channel(g), channel(b))
    }

    /// The picker's colour, plus the hue to keep while the colour is a grey (which has none).
    #[derive(Clone, Debug, PartialEq)]
    pub struct Picker {
        hex: String,
        hue: f64,
    }

    impl Picker {
        pub fn open(hex: &str) -> Self {
            let hex = hex.replace('#', "").to_uppercase();
            Self {
                hue: hsv(&hex).0,
                hex,
            }
        }

        pub fn hex(&self) -> &str {
            &self.hex
        }

        pub fn hue(&self) -> f64 {
            self.hue
        }

        /// Sets the colour. The hue updates only if `next` is not a grey.
        pub fn set(&mut self, next: &str) {
            self.hex = next.to_uppercase();
            let (hue, saturation, _) = hsv(&self.hex);
            if saturation > 0.0 {
                self.hue = hue;
            }
        }

        /// Sets saturation and brightness from a point in the square (0 to 1 on each axis).
        pub fn drag_square(&mut self, across: f64, down: f64) {
            let next = hex(
                self.hue,
                across.clamp(0.0, 1.0),
                (1.0 - down).clamp(0.0, 1.0),
            );
            self.set(&next);
        }

        /// Sets the hue from a point `across` the bar (0 to 1).
        pub fn drag_hue(&mut self, across: f64) {
            self.hue = (across * 360.0).clamp(0.0, LAST_HUE);
            let (_, saturation, brightness) = hsv(&self.hex);
            let next = hex(self.hue, saturation, brightness);
            self.set(&next);
        }

        /// Applies each keystroke in a channel field (0 red, 1 green, 2 blue) immediately if it
        /// parses as a number. Returns whether the colour changed.
        pub fn type_channel(&mut self, channel: usize, typed: &str) -> bool {
            let Some(next) = with_channel(&self.hex, channel, typed) else {
                return false;
            };
            self.set(&next);
            true
        }

        /// On leaving the hex field, takes the first six hex digits typed, dropping other
        /// characters. Fewer than six leaves the colour. Returns whether it was taken.
        pub fn commit_hex(&mut self, typed: &str) -> bool {
            let clean: String = typed
                .chars()
                .filter(char::is_ascii_hexdigit)
                .take(HEX_DIGITS)
                .collect();
            if clean.len() != HEX_DIGITS {
                return false;
            }
            self.set(&clean);
            true
        }

        /// Sets a colour from the eyedropper, channels 0 to 1.
        pub fn sampled(&mut self, red: f64, green: f64, blue: f64) {
            let channel = |value: f64| (value * 255.0).round() as i64;
            let next = format!(
                "{:02X}{:02X}{:02X}",
                channel(red),
                channel(green),
                channel(blue)
            );
            self.set(&next);
        }
    }

    /// Hex field text while typing: upper case, at most six characters.
    pub fn typed_hex(typed: &str) -> String {
        typed.to_uppercase().chars().take(HEX_DIGITS).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_appearance_page_offers_the_theme_then_each_modes_colours() {
        use crate::settings::page::Choice as Named;
        let mut settings = Settings::default();
        let page = settings.page("en");
        assert_eq!(
            (page.title.as_str(), page.summary.is_some()),
            ("Appearance", true)
        );
        let labels: Vec<&str> = page
            .sections
            .iter()
            .map(|section| section.label.as_str())
            .collect();
        assert_eq!(labels, ["Theme", "Light theme", "Dark theme"]);
        assert_eq!(
            page.sections[0].action, None,
            "the theme's section has no button"
        );
        assert_eq!(
            page.sections[0].rows[0].controls,
            [Control::Theme {
                action: THEME_ACTION.into(),
                labels: vec!["Light".into(), "Dark".into(), "System".into()],
                selected: 2
            }]
        );
        let Some(Control::Select {
            action,
            choices,
            selected,
            width,
            ..
        }) = &page.sections[2].action
        else {
            panic!("the preset's dropdown")
        };
        assert_eq!(
            (
                action.as_str(),
                choices.len(),
                choices[*selected].id.as_str(),
                *width
            ),
            ("appearance.dark.preset", 6, "northroom", 160.0)
        );
        let accent = &page.sections[2].rows[0];
        let Control::Select {
            choices,
            selected,
            width,
            ..
        } = &accent.controls[0]
        else {
            panic!("the accent's dropdown")
        };
        // A monochrome accent uses the theme's text colour.
        assert_eq!(
            choices[0],
            Named {
                id: "neutral".into(),
                name: "Monochrome".into(),
                swatch: Some("#EEEEF0".into())
            }
        );
        assert_eq!((choices[*selected].id.as_str(), *width), ("neutral", 128.0));
        assert_eq!(
            accent.controls[1],
            Control::Color {
                action: "appearance.dark.accent-color".into(),
                hex: "#EEEEF0".into()
            }
        );
        assert_eq!(
            page.sections[1].rows[4].controls,
            [Control::Slider {
                action: "appearance.light.contrast".into(),
                value: 50.0
            }]
        );

        // A custom colour makes both the preset and the accent custom.
        assert!(settings.act("appearance.dark.accent-color", "#123456"));
        let page = settings.page("en");
        assert_eq!(
            page.sections[0].action, None,
            "no reset button; the presets put a theme back"
        );
        let Some(Control::Select {
            choices, selected, ..
        }) = &page.sections[2].action
        else {
            panic!("the preset's dropdown")
        };
        assert_eq!(
            (
                choices.len(),
                choices[*selected].id.as_str(),
                choices[*selected].name.as_str()
            ),
            (7, "custom", "Custom")
        );
        let Control::Select {
            choices, selected, ..
        } = &page.sections[2].rows[0].controls[0]
        else {
            panic!("the accent's dropdown")
        };
        assert_eq!(
            (
                choices[*selected].id.as_str(),
                choices[*selected].swatch.as_ref()
            ),
            ("custom", None)
        );
        assert_eq!(
            settings.page("en").sections[0].rows[0].label,
            crate::strings::text("en", key::ACCOUNT_THEME_LABEL)
        );
        let english = choices_named(&settings.page("en"));
        let names: Vec<&str> = english.iter().map(|choice| choice.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Northroom",
                "Terminal",
                "Nord",
                "Rosé Pine",
                "Gruvbox",
                "Solarized"
            ]
        );
    }

    /// The light section's preset dropdown choices.
    fn choices_named(page: &Page) -> Vec<crate::settings::page::Choice> {
        let Some(Control::Select { choices, .. }) = &page.sections[1].action else {
            panic!("the preset's dropdown")
        };
        choices.clone()
    }

    #[test]
    fn the_appearance_page_acts_on_the_mode_its_control_names() {
        let mut settings = Settings::default();
        assert!(settings.act(THEME_ACTION, "1"));
        assert_eq!(
            (settings.theme, settings.shown_choice()),
            (Choice::Dark, Choice::Dark)
        );
        assert!(!settings.act(THEME_ACTION, "3"), "no fourth choice");
        assert!(settings.act("appearance.light.preset", "nord"));
        assert_eq!(
            (
                settings.light.preset.as_str(),
                settings.dark.preset.as_str()
            ),
            ("nord", "northroom")
        );
        assert!(!settings.act("appearance.light.preset", "sepia"));
        assert!(settings.act("appearance.dark.accent", "teal"));
        assert_eq!(settings.dark.accent_color, "#009B9B");
        assert!(
            !settings.act("appearance.dark.background", "#12345"),
            "a malformed colour changes nothing"
        );
        assert!(!settings.act("appearance.dark.cta", "red"));
        assert!(settings.act("appearance.dark.background", "#0A0B0C"));
        assert!(settings.act("appearance.dark.foreground", "#FAFAFA"));
        assert!(settings.act("appearance.dark.cta", "#FF0000"));
        assert!(settings.act("appearance.light.contrast", "62.4"));
        assert_eq!(settings.light.contrast, 62.0);
        assert!(!settings.act("appearance.light.contrast", "loud"));
        assert!(!settings.act("appearance.sepia.background", "#000000"));
        assert!(!settings.act("appearance.reset", ""), "there is no reset");
    }

    #[test]
    fn the_slider_writes_its_contrast_as_a_whole_number() {
        assert_eq!(
            (
                contrast_words(50.0),
                contrast_words(0.0),
                contrast_words(99.6)
            ),
            ("50".into(), "0".into(), "99".into())
        );
        assert_eq!(contrast_words(contrast_at(0.624)), "62");
    }

    #[test]
    fn a_preset_sets_the_colours_and_names_its_accent() {
        let mut theme = ThemeChoice::shipped(Mode::Dark);
        assert_eq!(
            (theme.background.as_str(), theme.accent_preset.as_str()),
            ("#111111", "neutral")
        );
        assert!(theme.apply_preset("terminal", Mode::Dark));
        assert_eq!(theme.background, "#0B110D");
        assert_eq!(
            (theme.accent_color.as_str(), theme.accent_preset.as_str()),
            ("#30A054", "green")
        );
        assert_eq!(
            theme.cta_color, "#DFECE1",
            "the call to action stays the text colour"
        );
        assert!(theme.apply_preset("gruvbox", Mode::Light));
        assert_eq!(
            (theme.accent_color.as_str(), theme.accent_preset.as_str()),
            ("#D65D0E", "custom"),
            "a palette's own accent has no name"
        );
        assert!(!theme.apply_preset("custom", Mode::Light));
    }

    #[test]
    fn an_edit_by_hand_ends_the_preset_and_monochrome_follows_the_text() {
        let mut theme = ThemeChoice::shipped(Mode::Light);
        theme.set_foreground("#202020");
        assert_eq!(theme.preset, "custom");
        assert_eq!(
            (theme.accent_color.as_str(), theme.cta_color.as_str()),
            ("#202020", "#202020")
        );
        theme.set_accent_color("#e34a49");
        assert_eq!(
            theme.accent_preset, "red",
            "a typed hex shows as its name in any case"
        );
        theme.set_foreground("#101010");
        assert_eq!(theme.accent_color, "#e34a49", "a coloured accent stays");
        theme.set_accent_color("#123456");
        assert_eq!(theme.accent_preset, "custom");
        assert!(theme.set_accent("neutral"));
        assert_eq!(theme.accent_color, "#101010");
        theme.set_contrast(140.0);
        assert_eq!(theme.contrast, 100.0);
    }

    #[test]
    fn choices_are_kept_and_read_back() {
        let directory = tempfile::tempdir().expect("directory");
        let file = directory.path().join("settings.json");
        std::fs::write(&file, "{ \"drive\": \"/a\" }").expect("another key");
        let mut settings = Settings::open(&file);
        assert_eq!(settings.theme, Choice::System);
        assert_eq!(settings.dark, ThemeChoice::shipped(Mode::Dark));
        assert_eq!(settings.mode(true), Mode::Dark);
        settings.theme = Choice::Light;
        settings.dark.apply_preset("nord", Mode::Dark);
        settings.save().expect("save");

        let again = Settings::open(&file);
        assert_eq!(again.theme, Choice::Light);
        assert_eq!(again.dark.preset, "nord");
        assert_eq!(again.mode(true), Mode::Light);
        let kept = crate::settings::file::read(&file).expect("settings");
        assert_eq!(kept["drive"], "/a", "the other keys are left as they were");
        assert_eq!(kept["appearance"]["theme"], "light");
    }

    #[test]
    fn a_file_that_names_only_the_theme_keeps_the_shipped_colours() {
        let directory = tempfile::tempdir().expect("directory");
        let file = directory.path().join("settings.json");
        std::fs::write(&file, "{ \"appearance\": { \"theme\": \"light\" } }").expect("written");
        let settings = Settings::read(&file).expect("settings");
        assert_eq!(settings.theme, Choice::Light);
        assert_eq!(settings.dark, ThemeChoice::shipped(Mode::Dark));
    }

    #[test]
    fn a_broken_settings_file_gives_the_shipped_look_and_is_left_alone() {
        let directory = tempfile::tempdir().expect("directory");
        let file = directory.path().join("settings.json");
        std::fs::write(&file, "{ broken").expect("broken");
        assert!(Settings::read(&file).is_err());
        let mut settings = Settings::open(&file);
        assert_eq!(settings.theme, Choice::System);
        settings.theme = Choice::Dark;
        assert!(
            settings.save().is_err(),
            "a broken file is not written over"
        );
        assert_eq!(std::fs::read_to_string(&file).expect("text"), "{ broken");
    }
    #[test]
    fn the_contrast_slider_steps_in_whole_numbers_within_its_track() {
        assert_eq!(contrast_at(0.504), 50.0);
        assert_eq!(contrast_at(0.506), 51.0);
        assert_eq!(contrast_at(-0.2), 0.0);
        assert_eq!(contrast_at(1.4), 100.0);
    }

    #[test]
    fn a_switch_thumb_follows_the_theme_and_the_switch() {
        use crate::theme::{switch_thumb, Token};
        assert_eq!(switch_thumb(false, true), Token::Background);
        assert_eq!(switch_thumb(false, false), Token::Background);
        assert_eq!(switch_thumb(true, true), Token::PrimaryForeground);
        assert_eq!(switch_thumb(true, false), Token::Foreground);
    }

    #[test]
    fn a_checks_look_shows_as_chosen_until_a_choice_is_made() {
        let mut settings = Settings::default();
        assert_eq!(settings.shown_choice(), Choice::System);
        settings.check_choice(Some(false));
        assert_eq!(
            (settings.shown_choice(), settings.theme),
            (Choice::Light, Choice::System)
        );
        settings.check_choice(Some(true));
        assert_eq!(settings.shown_choice(), Choice::Dark);
        settings.choose(Choice::Light);
        assert_eq!(
            (settings.shown_choice(), settings.theme),
            (Choice::Light, Choice::Light)
        );
        settings.choose(Choice::System);
        assert_eq!(
            settings.shown_choice(),
            Choice::System,
            "the check's look is gone once a choice is made"
        );
        settings.check_choice(None);
        assert_eq!(settings.shown_choice(), Choice::System);
    }

    #[test]
    fn a_typed_channel_is_read_as_parse_int_reads_it() {
        assert_eq!(with_channel("111113", 0, "200").as_deref(), Some("C81113"));
        assert_eq!(with_channel("111113", 1, " 7px").as_deref(), Some("110713"));
        assert_eq!(with_channel("#111113", 2, "999").as_deref(), Some("1111FF"));
        assert_eq!(with_channel("111113", 2, "-4").as_deref(), Some("111100"));
        assert_eq!(with_channel("111113", 0, "+12").as_deref(), Some("0C1113"));
        assert_eq!(
            with_channel("111113", 0, "99999999999999999999999").as_deref(),
            Some("FF1113")
        );
        assert_eq!(with_channel("111113", 0, ""), None);
        assert_eq!(with_channel("111113", 0, "x12"), None);
        assert_eq!(with_channel("111113", 3, "12"), None);
        assert_eq!(with_channel("11111", 0, "12"), None);
    }

    #[test]
    fn the_picker_reads_hex_and_writes_it_back() {
        use picker::{hex, hsv, rgb};
        assert_eq!(rgb("#4382F2"), [0x43, 0x82, 0xF2]);
        assert_eq!(rgb("43"), [0x43, 0, 0], "a missing channel is 0");
        assert_eq!(rgb("zz8200"), [0, 0x82, 0]);
        let (hue, saturation, brightness) = hsv("4382F2");
        assert!(
            (hue - 218.4).abs() < 0.1
                && (saturation - 0.7231).abs() < 1e-3
                && (brightness - 0.949).abs() < 1e-3
        );
        assert_eq!(hsv("808080"), (0.0, 0.0, 128.0 / 255.0));
        assert_eq!(hsv("FF0000"), (0.0, 1.0, 1.0));
        assert_eq!(hsv("FF00FF").0, 300.0);
        for color in [
            "4382F2", "E34A49", "000000", "FFFFFF", "30A054", "D64B97", "123456",
        ] {
            let (hue, saturation, brightness) = hsv(color);
            assert_eq!(
                hex(hue, saturation, brightness),
                color,
                "round trip of {color}"
            );
        }
        assert_eq!(hex(360.0, 1.0, 1.0), "000000", "past the bar is no colour");
        assert_eq!(hex(359.999, 1.0, 1.0), "FF0000");
    }

    #[test]
    fn the_picker_keeps_a_hue_for_a_grey_and_drags_within_its_square_and_bar() {
        let mut picker = picker::Picker::open("#e34a49");
        assert_eq!(picker.hex(), "E34A49");
        picker.drag_square(0.0, 1.0);
        assert_eq!(picker.hex(), "000000");
        assert!(
            (picker.hue() - 0.3896).abs() < 1e-3,
            "black keeps the hue it had: {}",
            picker.hue()
        );
        picker.drag_square(2.0, -1.0);
        assert_eq!(picker.hex(), "FF0200", "outside the square is its edge");
        picker.drag_hue(1.5);
        assert_eq!(picker.hex(), "FF0000");
        assert_eq!(
            picker.hue(),
            0.0,
            "a colour with a hue of its own gives the bar its hue"
        );
        picker.drag_hue(0.5);
        assert_eq!(picker.hex(), "00FFFF");
        picker.set("808080");
        assert_eq!((picker.hex(), picker.hue()), ("808080", 180.0));
    }

    #[test]
    fn the_pickers_fields_take_what_is_typed() {
        let mut picker = picker::Picker::open("111113");
        assert!(!picker.commit_hex("12ab"));
        assert_eq!(picker.hex(), "111113");
        assert!(picker.commit_hex("#12-ab-cd-ef"));
        assert_eq!(picker.hex(), "12ABCD");
        assert!(picker.type_channel(0, "255"));
        assert_eq!(picker.hex(), "FFABCD");
        assert!(!picker.type_channel(1, "x"));
        picker.sampled(1.0, 0.5, 0.0);
        assert_eq!(picker.hex(), "FF8000");
        assert_eq!(picker::typed_hex("abcdef12"), "ABCDEF");
        assert_eq!(picker::PRESETS.len(), 56);
    }
}
