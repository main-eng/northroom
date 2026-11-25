//! PDF's blend modes (ISO 32000 11.3.5): how a colour painted over another mixes with it.
//! Each takes the backdrop's and the source's colours, unpremultiplied, and gives the blended
//! colour; the canvas weighs that by both alphas (`raster::Canvas`).

/// A blend mode, from a graphics state's `BM`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Blend {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

type Rgb = [f32; 3];

impl Blend {
    /// The mode a PDF names; an unknown name is `Normal`, as the specification asks.
    pub fn named(name: &str) -> Self {
        match name {
            "Multiply" => Self::Multiply,
            "Screen" => Self::Screen,
            "Overlay" => Self::Overlay,
            "Darken" => Self::Darken,
            "Lighten" => Self::Lighten,
            "ColorDodge" => Self::ColorDodge,
            "ColorBurn" => Self::ColorBurn,
            "HardLight" => Self::HardLight,
            "SoftLight" => Self::SoftLight,
            "Difference" => Self::Difference,
            "Exclusion" => Self::Exclusion,
            "Hue" => Self::Hue,
            "Saturation" => Self::Saturation,
            "Color" => Self::Color,
            "Luminosity" => Self::Luminosity,
            _ => Self::Normal,
        }
    }

    /// The blended colour of `source` over `backdrop`.
    pub fn mix(self, backdrop: Rgb, source: Rgb) -> Rgb {
        match self {
            Self::Hue => with_luminosity(
                with_saturation(source, saturation(backdrop)),
                luminosity(backdrop),
            ),
            Self::Saturation => with_luminosity(
                with_saturation(backdrop, saturation(source)),
                luminosity(backdrop),
            ),
            Self::Color => with_luminosity(source, luminosity(backdrop)),
            Self::Luminosity => with_luminosity(backdrop, luminosity(source)),
            separable => {
                [0, 1, 2].map(|channel| separable.channel(backdrop[channel], source[channel]))
            }
        }
    }

    /// A separable mode on one channel.
    fn channel(self, b: f32, s: f32) -> f32 {
        match self {
            Self::Multiply => b * s,
            Self::Screen => b + s - b * s,
            Self::Overlay => Self::HardLight.channel(s, b),
            Self::Darken => b.min(s),
            Self::Lighten => b.max(s),
            Self::ColorDodge => colour_dodge(b, s),
            Self::ColorBurn => colour_burn(b, s),
            Self::HardLight if s <= 0.5 => b * 2.0 * s,
            Self::HardLight => Self::Screen.channel(b, 2.0 * s - 1.0),
            Self::SoftLight => soft_light(b, s),
            Self::Difference => (b - s).abs(),
            Self::Exclusion => b + s - 2.0 * b * s,
            _ => s,
        }
    }
}

fn colour_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

fn colour_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

fn soft_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
    }
    let d = if b <= 0.25 {
        ((16.0 * b - 12.0) * b + 4.0) * b
    } else {
        b.sqrt()
    };
    b + (2.0 * s - 1.0) * (d - b)
}

fn luminosity(colour: Rgb) -> f32 {
    0.3 * colour[0] + 0.59 * colour[1] + 0.11 * colour[2]
}

/// `colour` moved to luminosity `target`, brought back into range keeping its luminosity.
fn with_luminosity(colour: Rgb, target: f32) -> Rgb {
    let shift = target - luminosity(colour);
    let moved = colour.map(|channel| channel + shift);
    let l = luminosity(moved);
    let (low, high) = (
        moved.iter().copied().fold(f32::MAX, f32::min),
        moved.iter().copied().fold(f32::MIN, f32::max),
    );
    moved.map(|channel| {
        if low < 0.0 {
            l + (channel - l) * l / (l - low).max(1e-6)
        } else if high > 1.0 {
            l + (channel - l) * (1.0 - l) / (high - l).max(1e-6)
        } else {
            channel
        }
    })
}

fn saturation(colour: Rgb) -> f32 {
    colour.iter().copied().fold(f32::MIN, f32::max)
        - colour.iter().copied().fold(f32::MAX, f32::min)
}

/// `colour` with saturation `target`, its hue kept.
fn with_saturation(colour: Rgb, target: f32) -> Rgb {
    let (low, high) = (
        colour.iter().copied().fold(f32::MAX, f32::min),
        colour.iter().copied().fold(f32::MIN, f32::max),
    );
    if high - low <= 0.0 {
        return [0.0; 3];
    }
    colour.map(|channel| (channel - low) * target / (high - low))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separable_modes_mix_each_channel() {
        let (backdrop, source) = ([0.5, 0.2, 1.0], [0.5, 1.0, 0.0]);
        assert_eq!(Blend::Multiply.mix(backdrop, source), [0.25, 0.2, 0.0]);
        assert_eq!(Blend::Screen.mix(backdrop, source), [0.75, 1.0, 1.0]);
        assert_eq!(Blend::Darken.mix(backdrop, source), [0.5, 0.2, 0.0]);
        assert_eq!(Blend::Difference.mix(backdrop, source), [0.0, 0.8, 1.0]);
        assert_eq!(Blend::Normal.mix(backdrop, source), source);
        assert_eq!(Blend::named("Unknown"), Blend::Normal);
    }

    #[test]
    fn luminosity_keeps_the_backdrops_hue() {
        // Gray's luminosity on red keeps red's hue at gray's brightness.
        let mixed = Blend::Luminosity.mix([1.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
        assert!((luminosity(mixed) - 0.5).abs() < 1e-4, "{mixed:?}");
        assert!(mixed[0] > mixed[1] && mixed[1] == mixed[2]);
    }
}
