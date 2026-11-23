//! Colours and type. Colours come from the core's theme by `Token` (`nr_theme_color`); faces are
//! the bundled Inter and Geist Mono.

use crate::core::c;
use crate::nr::{NrCardFont, NrColor, nr_hex_color, nr_theme_color, nr_theme_revision};
use gpui::{Font, FontFeatures, FontStyle, FontWeight, Hsla, Rgba, SharedString};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

pub use northroom_core::theme::Token;

thread_local! {
    /// Cached colours and the theme revision they were read at (`nr_theme_revision`), so a
    /// theme change from outside the window, such as a hand-edited settings file, rereads them.
    static COLORS: RefCell<(u64, HashMap<Token, Hsla>)> = RefCell::new((0, HashMap::new()));
}

/// Corner radius for every control.
pub const CONTROL_RADIUS: f32 = 6.0;

fn rgba(color: NrColor) -> Rgba {
    Rgba {
        r: color.red,
        g: color.green,
        b: color.blue,
        a: color.alpha,
    }
}

/// The theme colour `token` in sRGB, as the core states it.
fn theme_color(token: Token) -> NrColor {
    let mut value = NrColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    };
    let name = c(token.name());
    // SAFETY: `name` is NUL-terminated and `value` is a valid out pointer.
    let found = unsafe { nr_theme_color(name.as_ptr(), &mut value) };
    assert!(found, "the core has no theme colour {token:?}");
    value
}

/// The theme colour `token` in the window's colour space (`crate::display`).
pub fn color(token: Token) -> Hsla {
    let revision = nr_theme_revision();
    let cached = COLORS.with(|colors| {
        let mut colors = colors.borrow_mut();
        if colors.0 != revision {
            *colors = (revision, HashMap::new());
        }
        colors.1.get(&token).copied()
    });
    if let Some(cached) = cached {
        return cached;
    }
    let color = crate::display::shown(rgba(theme_color(token)));
    COLORS.with(|colors| colors.borrow_mut().1.insert(token, color));
    color
}

/// A colour the core states in sRGB, in the window's colour space (`crate::display`).
pub fn shown(color: &NrColor) -> Hsla {
    crate::display::shown(Rgba {
        r: color.red,
        g: color.green,
        b: color.blue,
        a: color.alpha,
    })
}

/// The token the core names in a view.
fn token_named(name: &str) -> Token {
    Token::named(name).unwrap_or_else(|| panic!("the core named no theme colour {name}"))
}

/// `token` at `fraction` opacity over nothing: CSS's `color-mix(… N%, transparent)`.
pub fn mixed(token: Token, fraction: f32) -> Hsla {
    color(token).alpha(fraction)
}

/// `token` at `fraction` over the opaque `ground`, blended as Core Graphics does in AppKit's
/// backing store: each channel mixed from whole bytes and truncated to a byte. GPUI blends in
/// floating point and rounds, which makes faint shades one step lighter on dark grounds (the
/// dark list stripes come out 24 where AppKit draws 23).
pub fn laid_over(token: Token, fraction: f32, ground: Token) -> Hsla {
    composite(color(token).into(), color(ground).into(), fraction).into()
}

/// `over` at `fraction` on the opaque `under`, truncated to whole bytes.
fn composite(over: Rgba, under: Rgba, fraction: f32) -> Rgba {
    let channel = |over: f32, under: f32| {
        let (over, under) = ((over * 255.0).round(), (under * 255.0).round());
        // Nudge up so a mix that lands exactly on a byte isn't truncated to the one below.
        ((under * (1.0 - fraction) + over * fraction) + 1e-3).floor() / 255.0
    };
    Rgba {
        r: channel(over.r, under.r),
        g: channel(over.g, under.g),
        b: channel(over.b, under.b),
        a: 1.0,
    }
}

/// A colour the core names (a theme token or `#rrggbb`) in the window's colour space.
pub fn named(name: &str) -> Hsla {
    if name.starts_with('#') {
        crate::display::shown(stated_named(name))
    } else {
        color(token_named(name))
    }
}

/// The theme colour `token` in sRGB, for passing back to the core or the platform.
pub fn stated(token: Token) -> Rgba {
    rgba(theme_color(token))
}

/// A colour the core names (a theme token or `#rrggbb`) in sRGB.
pub fn stated_named(name: &str) -> Rgba {
    if name.starts_with('#') {
        let hex = c(name);
        // SAFETY: `hex` is NUL-terminated.
        rgba(unsafe { nr_hex_color(hex.as_ptr()) })
    } else {
        stated(token_named(name))
    }
}

/// `color` with its alpha multiplied by `opacity`.
pub fn faded(color: Hsla, opacity: f64) -> Hsla {
    if opacity == 1.0 {
        color
    } else {
        color.alpha(color.a * opacity as f32)
    }
}

/// Clears the colour cache after the appearance changes.
pub fn forget() {
    COLORS.with(|colors| colors.borrow_mut().1.clear());
}

/// A face as the core describes it (`NrCardFont`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Face {
    pub size: f32,
    pub weight: f32,
    pub mono: bool,
    pub tabular: bool,
    pub slanted: bool,
}

impl Face {
    /// Inter at `size` and a weight (400 regular, 500 medium, 700 bold).
    pub fn sans(size: f32, weight: f32) -> Self {
        Self {
            size,
            weight,
            mono: false,
            tabular: false,
            slanted: false,
        }
    }

    /// Geist Mono at `size`, regular. A view may ask for bold (700), as the source view does
    /// through AppKit's bold trait.
    pub fn mono(size: f32) -> Self {
        Self {
            size,
            weight: 400.0,
            mono: true,
            tabular: false,
            slanted: false,
        }
    }

    /// The same face with tabular digits (`font-variant-numeric: tabular-nums`).
    pub fn tabular(self) -> Self {
        Self {
            tabular: true,
            ..self
        }
    }

    /// A face the core names. The monospace face keeps its single weight.
    pub fn of(font: &NrCardFont) -> Self {
        Self {
            size: font.size as f32,
            weight: if font.mono { 400.0 } else { font.weight as f32 },
            mono: font.mono,
            tabular: font.tabular,
            slanted: font.slanted,
        }
    }

    pub fn font(&self) -> Font {
        let features = if self.tabular {
            FontFeatures(Arc::new(vec![("tnum".to_owned(), 1)]))
        } else {
            FontFeatures::default()
        };
        Font {
            family: SharedString::new_static(if self.mono { "Geist Mono" } else { "Inter" }),
            features,
            fallbacks: None,
            weight: FontWeight(self.weight),
            style: if self.slanted {
                FontStyle::Oblique
            } else {
                FontStyle::Normal
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn byte(value: u8) -> f32 {
        f32::from(value) / 255.0
    }

    fn bytes(color: Rgba) -> [u8; 3] {
        [color.r, color.g, color.b].map(|channel| (channel * 255.0).round() as u8)
    }

    #[test]
    fn a_faint_shade_is_mixed_in_whole_bytes_as_the_references_draw_it() {
        let gray = |r: u8, g: u8, b: u8| Rgba {
            r: byte(r),
            g: byte(g),
            b: byte(b),
            a: 1.0,
        };
        // List stripes are 3% of the text colour on the ground. AppKit draws 23 in dark
        // (rounding would give 24) and 243 in light.
        let dark = composite(gray(0xee, 0xee, 0xf0), gray(0x11, 0x11, 0x11), 0.03);
        assert_eq!(bytes(dark), [23, 23, 23]);
        let light = composite(gray(0x11, 0x11, 0x13), gray(0xfa, 0xfa, 0xfb), 0.03);
        assert_eq!(bytes(light), [243, 243, 244]);
        // 0% leaves the ground; 100% leaves the overlay.
        assert_eq!(
            bytes(composite(gray(200, 10, 0), gray(17, 17, 17), 0.0)),
            [17, 17, 17]
        );
    }
}
