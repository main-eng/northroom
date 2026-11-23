//! The appearance kernel: derives every chrome colour from a person's background, text, accent
//! and contrast choices. `tests/theme_parity.rs` pins it to recorded choices and their colours.
//!
//! The surface ladder keeps each rung's lightness distance from the ground, so panels stay
//! distinct on any background. Contrast floors are enforced by moving the value, never by
//! warning.

/// A colour in OKLCH: perceptual lightness 0–1, chroma, hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f64,
    pub c: f64,
    pub h: f64,
}

#[derive(Clone, Copy)]
struct LinearRgb {
    r: f64,
    g: f64,
    b: f64,
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn srgb_to_linear(channel: f64) -> f64 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(channel: f64) -> f64 {
    if channel <= 0.003_130_8 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    }
}

fn oklab_to_linear(l: f64, a: f64, b: f64) -> LinearRgb {
    let long = (l + 0.396_337_777_4 * a + 0.215_803_757_3 * b).powi(3);
    let medium = (l - 0.105_561_345_8 * a - 0.063_854_172_8 * b).powi(3);
    let short = (l - 0.089_484_177_5 * a - 1.291_485_548 * b).powi(3);
    LinearRgb {
        r: 4.076_741_662_1 * long - 3.307_711_591_3 * medium + 0.230_969_929_2 * short,
        g: -1.268_438_004_6 * long + 2.609_757_401_1 * medium - 0.341_319_396_5 * short,
        b: -0.004_196_086_3 * long - 0.703_418_614_7 * medium + 1.707_614_701 * short,
    }
}

/// Parses `#RRGGBB` (the `#` optional).
fn hex_to_linear(hex: &str) -> Option<LinearRgb> {
    let digits = hex.trim().trim_start_matches('#');
    if digits.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let channel = |shift: u32| srgb_to_linear(f64::from((value >> shift) & 0xff) / 255.0);
    Some(LinearRgb {
        r: channel(16),
        g: channel(8),
        b: channel(0),
    })
}

/// True when every channel lands inside sRGB, within a rounding epsilon.
fn is_displayable(color: LinearRgb) -> bool {
    const EPSILON: f64 = 1e-4;
    [color.r, color.g, color.b]
        .iter()
        .all(|channel| *channel >= -EPSILON && *channel <= 1.0 + EPSILON)
}

pub fn hex_to_oklch(hex: &str) -> Option<Oklch> {
    let LinearRgb { r, g, b } = hex_to_linear(hex)?;
    let long = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let medium = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let short = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    let l = 0.210_454_255_3 * long + 0.793_617_785 * medium - 0.004_072_046_8 * short;
    let a = 1.977_998_495_1 * long - 2.428_592_205 * medium + 0.450_593_709_9 * short;
    let b = 0.025_904_037_1 * long + 0.782_771_766_2 * medium - 0.808_675_766 * short;
    let chroma = a.hypot(b);
    // Below this, a and b are rounding noise and the hue angle means nothing: report 0 so a
    // neutral grey keeps a stable hue.
    let hue = if chroma < 1e-5 {
        0.0
    } else {
        b.atan2(a).to_degrees()
    };
    Some(Oklch {
        l: clamp01(l),
        c: chroma,
        h: (hue + 360.0) % 360.0,
    })
}

/// The colour as gamma-encoded sRGB bytes, reducing chroma until it fits in sRGB. Lightness
/// and hue are preserved.
pub fn oklch_to_srgb8(color: Oklch) -> [u8; 3] {
    let lightness = clamp01(color.l);
    let radians = color.h.to_radians();
    let project =
        |chroma: f64| oklab_to_linear(lightness, radians.cos() * chroma, radians.sin() * chroma);
    let mut displayable = project(color.c);
    if !is_displayable(displayable) {
        let (mut low, mut high) = (0.0, color.c.max(0.0));
        // 20 halvings resolve chroma far finer than an 8-bit channel can show.
        for _ in 0..20 {
            let middle = (low + high) / 2.0;
            if is_displayable(project(middle)) {
                low = middle;
            } else {
                high = middle;
            }
        }
        displayable = project(low);
    }
    // Halves round up.
    let byte =
        |channel: f64| (clamp01(linear_to_srgb(clamp01(channel))) * 255.0 + 0.5).floor() as u8;
    [
        byte(displayable.r),
        byte(displayable.g),
        byte(displayable.b),
    ]
}

fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let linear = |byte: u8| clamp01(srgb_to_linear(f64::from(byte) / 255.0));
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

/// WCAG 2.x contrast ratio, 1–21, between two colours as they display.
fn contrast_ratio(color: Oklch, against: Oklch) -> f64 {
    let a = relative_luminance(oklch_to_srgb8(color));
    let b = relative_luminance(oklch_to_srgb8(against));
    let (lighter, darker) = if a > b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

/// A person's choices for one mode. Colours are `#RRGGBB`; `contrast` is a 0–100 dial where 50
/// gives the default ladder.
#[derive(Clone, Debug)]
pub struct Appearance {
    pub background: String,
    pub foreground: String,
    pub accent_color: String,
    pub cta_color: String,
    pub contrast: f64,
}

impl Appearance {
    /// The default look for `mode`, equal to an uncustomised appearance.
    pub fn shipped(mode: Mode) -> Self {
        let (background, foreground) = match mode {
            Mode::Light => ("#FAFAFB", "#111113"),
            Mode::Dark => ("#111111", "#EEEEF0"),
        };
        Self {
            background: background.to_owned(),
            foreground: foreground.to_owned(),
            accent_color: foreground.to_owned(),
            cta_color: foreground.to_owned(),
            contrast: CONTRAST_NEUTRAL,
        }
    }
}

/// The default surface ladder: signed lightness offsets from the background per rung, negative
/// in light mode (deeper is darker) and positive in dark mode (raised is lighter).
struct Ladder {
    surface: f64,
    panel_header: f64,
    surface_raised: f64,
    muted: f64,
    field: f64,
    accent: f64,
    popover: f64,
    /// Where emphasis text sits relative to body text, toward the background.
    foreground_emphasis_offset: f64,
    /// Muted text as a fraction of the way from body text to the background.
    muted_foreground_fraction: f64,
    ink_lightness: f64,
    paper_lightness: f64,
    on_accent_lightness: f64,
}

const LIGHT_LADDER: Ladder = Ladder {
    surface: -0.025,
    panel_header: -0.0325,
    surface_raised: -0.04,
    muted: -0.05,
    field: -0.047,
    accent: -0.075,
    popover: -0.01,
    foreground_emphasis_offset: 0.04,
    muted_foreground_fraction: 0.3975,
    ink_lightness: 0.14,
    paper_lightness: 0.985,
    on_accent_lightness: 0.99,
};

const DARK_LADDER: Ladder = Ladder {
    surface: 0.018,
    panel_header: 0.0275,
    surface_raised: 0.037,
    muted: 0.057,
    field: 0.062,
    accent: 0.097,
    popover: 0.047,
    foreground_emphasis_offset: 0.0,
    muted_foreground_fraction: 0.3238,
    ink_lightness: 0.1,
    paper_lightness: 0.985,
    on_accent_lightness: 0.99,
};

const CONTRAST_NEUTRAL: f64 = 50.0;
const FOREGROUND_MINIMUM_RATIO: f64 = 7.0;
const MUTED_FOREGROUND_MINIMUM_RATIO: f64 = 4.5;
/// A fill keeps a paper label at or above this contrast ratio and switches to ink below it.
const ACCENT_LABEL_MINIMUM_RATIO: f64 = 3.5;
/// The call-to-action keeps its paper label down to this lower floor.
const CTA_PAPER_LABEL_MINIMUM_RATIO: f64 = 2.5;
const MINIMUM_SURFACE_LIGHTNESS: f64 = 0.1;
const MAXIMUM_SURFACE_LIGHTNESS: f64 = 0.99;
/// Borders are body text at this alpha.
pub const BORDER_ALPHA: f64 = 0.10;

const BLACK: Oklch = Oklch {
    l: 0.0,
    c: 0.0,
    h: 0.0,
};
const WHITE: Oklch = Oklch {
    l: 1.0,
    c: 0.0,
    h: 0.0,
};

/// The dial as a multiplier for ladder distances; 50 gives 1. The floor stays above zero so
/// the surfaces never collapse into one plane.
fn contrast_scale(contrast: f64) -> f64 {
    let dial = if contrast.is_finite() {
        contrast.clamp(0.0, 100.0)
    } else {
        0.0
    };
    if dial <= CONTRAST_NEUTRAL {
        0.35 + dial / CONTRAST_NEUTRAL * 0.65
    } else {
        1.0 + (dial - CONTRAST_NEUTRAL) / CONTRAST_NEUTRAL * 0.9
    }
}

fn minimum_contrast(color: Oklch, surfaces: &[Oklch]) -> f64 {
    surfaces
        .iter()
        .map(|surface| contrast_ratio(color, *surface))
        .fold(f64::INFINITY, f64::min)
}

/// Keeps the rung distances, reversing elevation when the chosen ground has no safe room.
fn surface_direction(background: Oklch, offset: f64) -> f64 {
    let candidate = Oklch {
        l: background.l + offset,
        ..background
    };
    if candidate.l < MINIMUM_SURFACE_LIGHTNESS || candidate.l > MAXIMUM_SURFACE_LIGHTNESS {
        return -1.0;
    }
    let surfaces = [background, candidate];
    let available = minimum_contrast(BLACK, &surfaces).max(minimum_contrast(WHITE, &surfaces));
    if available >= MUTED_FOREGROUND_MINIMUM_RATIO {
        1.0
    } else {
        -1.0
    }
}

/// Moves `color` toward whichever extreme reads better until it clears `minimum` on every
/// surface. Returns whether it moved.
fn raise_contrast(color: Oklch, surfaces: &[Oklch], minimum: f64) -> (Oklch, bool) {
    if minimum_contrast(color, surfaces) >= minimum {
        return (color, false);
    }
    let direction = if minimum_contrast(WHITE, surfaces) >= minimum_contrast(BLACK, surfaces) {
        0.01
    } else {
        -0.01
    };
    let mut candidate = color;
    for _ in 0..100 {
        let lightness = (candidate.l + direction).clamp(0.0, 1.0);
        if lightness == candidate.l {
            break;
        }
        candidate.l = lightness;
        if minimum_contrast(candidate, surfaces) >= minimum {
            break;
        }
    }
    (candidate, true)
}

/// Every chrome colour for one mode.
#[derive(Clone, Debug)]
pub struct Theme {
    pub paper: Oklch,
    pub ink: Oklch,
    pub on_accent: Oklch,
    pub background: Oklch,
    pub foreground: Oklch,
    /// `card` and `panel`.
    pub surface: Oklch,
    pub panel_header: Oklch,
    /// `frame`, `secondary` and `input`.
    pub surface_raised: Oklch,
    pub muted: Oklch,
    pub field: Oklch,
    /// The `accent` surface.
    pub accent: Oklch,
    pub popover: Oklch,
    /// `secondary-foreground` and `accent-foreground`.
    pub foreground_emphasis: Oklch,
    pub muted_foreground: Oklch,
    /// `primary` and `ring`.
    pub primary: Oklch,
    pub primary_foreground: Oklch,
    pub cta: Oklch,
    pub cta_foreground: Oklch,
    /// The colour for warnings and irreversible actions; fixed per mode.
    pub destructive: Oklch,
    /// True when a contrast floor moved a value away from the request.
    pub clamped: bool,
    /// The mode it was derived for, which decides whether views draw in the dark theme.
    pub mode: Mode,
}

/// A theme colour by role. At the boundary (a shell request, layout JSON) a token is named as a
/// CSS custom property without the leading dashes (`Token::name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Token {
    Paper,
    Ink,
    OnAccent,
    Background,
    Foreground,
    CardForeground,
    PopoverForeground,
    FieldForeground,
    Card,
    Panel,
    PanelHeader,
    Frame,
    Secondary,
    Input,
    Muted,
    Field,
    Accent,
    Popover,
    SecondaryForeground,
    AccentForeground,
    MutedForeground,
    Primary,
    Ring,
    StatusInfo,
    PrimaryForeground,
    Cta,
    CtaForeground,
    Destructive,
    /// A finished turn.
    StatusSuccess,
    /// A turn that needs the person.
    StatusWarning,
    /// Text on a `StatusWarning` ground.
    StatusWarningForeground,
    Border,
}

impl Token {
    pub const ALL: [Token; 32] = [
        Token::Paper,
        Token::Ink,
        Token::OnAccent,
        Token::Background,
        Token::Foreground,
        Token::CardForeground,
        Token::PopoverForeground,
        Token::FieldForeground,
        Token::Card,
        Token::Panel,
        Token::PanelHeader,
        Token::Frame,
        Token::Secondary,
        Token::Input,
        Token::Muted,
        Token::Field,
        Token::Accent,
        Token::Popover,
        Token::SecondaryForeground,
        Token::AccentForeground,
        Token::MutedForeground,
        Token::Primary,
        Token::Ring,
        Token::StatusInfo,
        Token::PrimaryForeground,
        Token::Cta,
        Token::CtaForeground,
        Token::Destructive,
        Token::StatusSuccess,
        Token::StatusWarning,
        Token::StatusWarningForeground,
        Token::Border,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Token::Paper => "paper",
            Token::Ink => "ink",
            Token::OnAccent => "on-accent",
            Token::Background => "background",
            Token::Foreground => "foreground",
            Token::CardForeground => "card-foreground",
            Token::PopoverForeground => "popover-foreground",
            Token::FieldForeground => "field-foreground",
            Token::Card => "card",
            Token::Panel => "panel",
            Token::PanelHeader => "panel-header",
            Token::Frame => "frame",
            Token::Secondary => "secondary",
            Token::Input => "input",
            Token::Muted => "muted",
            Token::Field => "field",
            Token::Accent => "accent",
            Token::Popover => "popover",
            Token::SecondaryForeground => "secondary-foreground",
            Token::AccentForeground => "accent-foreground",
            Token::MutedForeground => "muted-foreground",
            Token::Primary => "primary",
            Token::Ring => "ring",
            Token::StatusInfo => "status-info",
            Token::PrimaryForeground => "primary-foreground",
            Token::Cta => "cta",
            Token::CtaForeground => "cta-foreground",
            Token::Destructive => "destructive",
            Token::StatusSuccess => "status-success",
            Token::StatusWarning => "status-warning",
            Token::StatusWarningForeground => "status-warning-foreground",
            Token::Border => "border",
        }
    }

    pub fn named(name: &str) -> Option<Token> {
        Self::ALL.into_iter().find(|token| token.name() == name)
    }
}

/// A colour a view names: a theme token, or a fixed `#rrggbb`. At the boundary it is the token
/// name or the hex (`Paint::name`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    Theme(Token),
    Fixed(&'static str),
}

impl From<Token> for Paint {
    fn from(token: Token) -> Self {
        Paint::Theme(token)
    }
}

impl Paint {
    pub fn name(self) -> &'static str {
        match self {
            Paint::Theme(token) => token.name(),
            Paint::Fixed(hex) => hex,
        }
    }
}

/// The token for a switch thumb: the page colour on a light theme. On a dark theme, the text
/// colour when off and the accent's text colour when on.
pub fn switch_thumb(dark: bool, on: bool) -> Token {
    match (dark, on) {
        (false, _) => Token::Background,
        (true, true) => Token::PrimaryForeground,
        (true, false) => Token::Foreground,
    }
}

/// Derives the theme. `None` when a colour is not `#RRGGBB`.
pub fn derive(appearance: &Appearance, mode: Mode) -> Option<Theme> {
    let ladder = match mode {
        Mode::Light => &LIGHT_LADDER,
        Mode::Dark => &DARK_LADDER,
    };
    let scale = contrast_scale(appearance.contrast);

    let requested_background = hex_to_oklch(&appearance.background)?;
    // Keep visible range at both display extremes.
    let background = Oklch {
        l: requested_background
            .l
            .clamp(MINIMUM_SURFACE_LIGHTNESS, MAXIMUM_SURFACE_LIGHTNESS),
        ..requested_background
    };
    let requested_foreground = hex_to_oklch(&appearance.foreground)?;
    let direction = surface_direction(background, ladder.accent * scale);
    let available_range = if ladder.accent * direction > 0.0 {
        MAXIMUM_SURFACE_LIGHTNESS - background.l
    } else {
        background.l - MINIMUM_SURFACE_LIGHTNESS
    };
    let surface_scale = scale.min(available_range / ladder.accent.abs());

    // Surfaces share the ground's hue and chroma; only lightness separates them.
    let surface_at = |offset: f64| Oklch {
        l: background.l + offset * surface_scale * direction,
        ..background
    };
    let surface = surface_at(ladder.surface);
    let panel_header = surface_at(ladder.panel_header);
    let surface_raised = surface_at(ladder.surface_raised);
    let muted = surface_at(ladder.muted);
    let field = surface_at(ladder.field);
    let accent = surface_at(ladder.accent);
    let popover = surface_at(ladder.popover);
    let surfaces = [
        background,
        surface,
        panel_header,
        surface_raised,
        muted,
        field,
        accent,
        popover,
    ];

    let (foreground, foreground_moved) =
        raise_contrast(requested_foreground, &surfaces, FOREGROUND_MINIMUM_RATIO);
    // Zero stays zero, where `signum` would give one.
    let gap = background.l - foreground.l;
    let toward_background = if gap == 0.0 { 0.0 } else { gap.signum() };
    let (foreground_emphasis, _) = raise_contrast(
        Oklch {
            l: (foreground.l + ladder.foreground_emphasis_offset * toward_background)
                .clamp(0.0, 1.0),
            ..foreground
        },
        &surfaces,
        MUTED_FOREGROUND_MINIMUM_RATIO,
    );

    // Muted text sits a fraction of the way from body text toward the ground; a higher
    // contrast dial pulls it back toward body text.
    let muted_fraction = (ladder.muted_foreground_fraction / scale).clamp(0.05, 0.9);
    let requested_muted = Oklch {
        l: foreground.l + muted_fraction * (background.l - foreground.l),
        ..foreground
    };
    let (muted_foreground, muted_moved) =
        raise_contrast(requested_muted, &surfaces, MUTED_FOREGROUND_MINIMUM_RATIO);

    let on_accent = Oklch {
        l: ladder.on_accent_lightness,
        c: 0.002,
        h: background.h,
    };
    let paper = Oklch {
        l: ladder.paper_lightness,
        c: 0.002,
        h: background.h,
    };
    let ink = Oklch {
        l: ladder.ink_lightness,
        c: 0.003,
        h: background.h,
    };
    let primary = hex_to_oklch(&appearance.accent_color)?;
    let primary_foreground = label_on(primary, paper, ink, ACCENT_LABEL_MINIMUM_RATIO);
    let cta = hex_to_oklch(&appearance.cta_color)?;
    let cta_foreground = label_on(cta, paper, ink, CTA_PAPER_LABEL_MINIMUM_RATIO);

    Some(Theme {
        paper,
        ink,
        on_accent,
        background,
        foreground,
        surface,
        panel_header,
        surface_raised,
        muted,
        field,
        accent,
        popover,
        foreground_emphasis,
        muted_foreground,
        primary,
        primary_foreground,
        cta,
        cta_foreground,
        destructive: destructive(mode),
        clamped: background.l != requested_background.l
            || direction != 1.0
            || surface_scale != scale
            || foreground_moved
            || muted_moved,
        mode,
    })
}

/// Amber for a turn that needs the person, in every theme.
const STATUS_WARNING: Oklch = Oklch {
    l: 0.769,
    c: 0.188,
    h: 70.08,
};

/// Label colour on `fill`: paper when it reaches `minimum_ratio`, else ink.
fn label_on(fill: Oklch, paper: Oklch, ink: Oklch, minimum_ratio: f64) -> Oklch {
    if contrast_ratio(paper, fill) >= minimum_ratio {
        paper
    } else {
        ink
    }
}

/// Red for a destructive action, in every theme.
fn destructive(mode: Mode) -> Oklch {
    match mode {
        Mode::Light => Oklch {
            l: 0.577,
            c: 0.245,
            h: 27.325,
        },
        Mode::Dark => Oklch {
            l: 0.704,
            c: 0.191,
            h: 22.216,
        },
    }
}

impl Theme {
    /// A token's colour and its opacity.
    pub fn token(&self, token: Token) -> (Oklch, f64) {
        let opaque = |color: Oklch| (color, 1.0);
        match token {
            Token::Paper => opaque(self.paper),
            Token::Ink => opaque(self.ink),
            Token::OnAccent => opaque(self.on_accent),
            Token::Background => opaque(self.background),
            Token::Foreground
            | Token::CardForeground
            | Token::PopoverForeground
            | Token::FieldForeground => opaque(self.foreground),
            Token::Card | Token::Panel => opaque(self.surface),
            Token::PanelHeader => opaque(self.panel_header),
            Token::Frame | Token::Secondary | Token::Input => opaque(self.surface_raised),
            Token::Muted => opaque(self.muted),
            Token::Field => opaque(self.field),
            Token::Accent => opaque(self.accent),
            Token::Popover => opaque(self.popover),
            Token::SecondaryForeground | Token::AccentForeground => {
                opaque(self.foreground_emphasis)
            }
            Token::MutedForeground => opaque(self.muted_foreground),
            Token::Primary | Token::Ring => opaque(self.primary),
            // A working session keeps the text colour whatever the accent, as other statuses
            // keep theirs. A green accent would read as finished.
            Token::StatusInfo => opaque(self.foreground),
            Token::PrimaryForeground => opaque(self.primary_foreground),
            Token::Cta => opaque(self.cta),
            Token::CtaForeground => opaque(self.cta_foreground),
            Token::Destructive => opaque(self.destructive),
            Token::StatusSuccess => opaque(Oklch {
                l: 0.696,
                c: 0.17,
                h: 162.48,
            }),
            Token::StatusWarning => opaque(STATUS_WARNING),
            Token::StatusWarningForeground => opaque(label_on(
                STATUS_WARNING,
                self.paper,
                self.ink,
                CTA_PAPER_LABEL_MINIMUM_RATIO,
            )),
            Token::Border => (self.foreground, BORDER_ALPHA),
        }
    }
}

#[cfg(test)]
mod token_tests {
    use super::{derive, Appearance, Mode, Token};

    #[test]
    fn a_session_at_work_keeps_the_text_colour_whatever_the_accent() {
        for mode in [Mode::Light, Mode::Dark] {
            let shipped = derive(&Appearance::shipped(mode), mode).expect("the shipped look");
            // The brand is monochrome: the accent and a session at work are both the text.
            assert_eq!(
                shipped.token(Token::StatusInfo),
                shipped.token(Token::Foreground)
            );
            assert_eq!(
                shipped.token(Token::Primary).0,
                shipped.token(Token::Foreground).0
            );
            let green = derive(
                &Appearance {
                    accent_color: "#30A054".to_owned(),
                    ..Appearance::shipped(mode)
                },
                mode,
            )
            .expect("a green accent");
            assert_ne!(green.token(Token::Primary), green.token(Token::Foreground));
            assert_eq!(
                green.token(Token::StatusInfo),
                green.token(Token::Foreground)
            );
        }
    }

    #[test]
    fn every_token_reads_back_from_its_name() {
        for token in Token::ALL {
            assert_eq!(Token::named(token.name()), Some(token));
        }
        assert_eq!(Token::named("--foreground"), None);
    }
}
