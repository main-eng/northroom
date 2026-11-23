//! Checks the theme kernel against recorded colours (`fixtures/theme.json`) for the same
//! choices.

use northroom_core::theme::{derive, oklch_to_srgb8, Appearance, Mode, Token};
use serde_json::Value;

/// The fixture rounds lightness and chroma to four places and hue to three.
const LIGHTNESS_TOLERANCE: f64 = 0.000_051;
const HUE_TOLERANCE: f64 = 0.000_51;

#[test]
fn derives_the_recorded_colours() {
    let cases: Value = serde_json::from_str(include_str!("fixtures/theme.json")).expect("fixture");
    let cases = cases.as_array().expect("an array of cases");
    assert!(cases.len() >= 20);
    for case in cases {
        let text = |key: &str| case[key].as_str().expect(key).to_owned();
        let appearance = Appearance {
            background: text("background"),
            foreground: text("foreground"),
            accent_color: text("accentColor"),
            cta_color: text("ctaColor"),
            contrast: case["contrast"].as_f64().expect("contrast"),
        };
        let mode = if text("mode") == "dark" {
            Mode::Dark
        } else {
            Mode::Light
        };
        let label = format!("{appearance:?} {mode:?}");
        let theme = derive(&appearance, mode).expect("valid colours");
        assert_eq!(
            theme.clamped,
            case["clamped"].as_bool().expect("clamped"),
            "clamped, {label}"
        );
        for (name, expected) in case["tokens"].as_object().expect("tokens") {
            let token = Token::named(name).unwrap_or_else(|| panic!("no token {name}"));
            let (color, alpha) = theme.token(token);
            let number = |key: &str| expected[key].as_f64().expect(key);
            assert!(
                (color.l.clamp(0.0, 1.0) - number("l")).abs() <= LIGHTNESS_TOLERANCE,
                "{name} lightness {} vs {}, {label}",
                color.l,
                number("l")
            );
            assert!(
                (color.c.max(0.0) - number("c")).abs() <= LIGHTNESS_TOLERANCE,
                "{name} chroma {} vs {}, {label}",
                color.c,
                number("c")
            );
            let hue = color.h.rem_euclid(360.0);
            let hue_gap = (hue - number("h")).abs();
            assert!(
                hue_gap <= HUE_TOLERANCE || (360.0 - hue_gap) <= HUE_TOLERANCE,
                "{name} hue {hue} vs {}, {label}",
                number("h")
            );
            assert!(
                (alpha - number("alpha")).abs() < 1e-9,
                "{name} alpha, {label}"
            );
            // The displayed colour of the recorded value.
            let printed = northroom_core::theme::Oklch {
                l: number("l"),
                c: number("c"),
                h: number("h"),
            };
            let [red, green, blue] = oklch_to_srgb8(printed);
            assert_eq!(
                format!("#{red:02x}{green:02x}{blue:02x}"),
                expected["hex"].as_str().expect("hex"),
                "{name} hex, {label}"
            );
        }
    }
}
