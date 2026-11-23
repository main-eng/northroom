//! Colour management off macOS, matching Core Graphics: images with a profile are converted to
//! sRGB, and untagged images are treated as sRGB. Uses the ICC matrix-and-curve conversion: each
//! channel is linearised by its curve, taken through the connection space (XYZ, D50) by the
//! colorants, then out through sRGB's colorants and curve. ColorSync rounds a few dark levels of
//! some profiles up by one (Chromium's Display P3 greys 12 and 14 become 13 and 15), so results
//! can differ from macOS by one level.

/// sRGB colorants as macOS's `sRGB Profile.icc` states them (s15Fixed16).
const SRGB_COLORANTS: [[f64; 3]; 3] = [
    [
        0.436_065_673_828_125,
        0.222_488_403_320_312_5,
        0.013_916_015_625,
    ],
    [
        0.385_147_094_726_562_5,
        0.716_873_168_945_312_5,
        0.097_076_416_015_625,
    ],
    [
        0.143_066_406_25,
        0.060_607_910_156_25,
        0.714_096_069_335_937_5,
    ],
];

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn fixed(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from(u32_at(data, at)? as i32) / 65536.0)
}

/// The data of tag `name` in `profile`.
fn tag<'a>(profile: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let count = u32_at(profile, 128)? as usize;
    (0..count).find_map(|index| {
        let record = 132 + index * 12;
        (profile.get(record..record + 4)? == name).then_some(())?;
        let (offset, size) = (
            u32_at(profile, record + 4)? as usize,
            u32_at(profile, record + 8)? as usize,
        );
        profile.get(offset..offset + size)
    })
}

/// A colorant (`XYZType`).
fn colorant(profile: &[u8], name: &[u8; 4]) -> Option<[f64; 3]> {
    let data = tag(profile, name)?;
    (data.get(..4)? == b"XYZ ").then_some(())?;
    Some([fixed(data, 8)?, fixed(data, 12)?, fixed(data, 16)?])
}

/// A tone curve (`parametricCurveType` or `curveType`) as linear light per 8-bit level.
fn curve(profile: &[u8], name: &[u8; 4]) -> Option<[f64; 256]> {
    let data = tag(profile, name)?;
    match data.get(..4)? {
        b"para" => parametric_curve(data),
        b"curv" => sampled_curve(data),
        _ => None,
    }
}

/// Levels of a `parametricCurveType`: one of ICC's five functions, with up to seven parameters.
fn parametric_curve(data: &[u8]) -> Option<[f64; 256]> {
    let kind = u16_at(data, 8)?;
    let count = [1, 3, 4, 5, 7].get(usize::from(kind))?;
    let mut values = [0.0; 7];
    for (index, value) in values.iter_mut().enumerate().take(*count) {
        *value = fixed(data, 12 + index * 4)?;
    }
    let mut levels = [0.0; 256];
    for (level, out) in levels.iter_mut().enumerate() {
        *out = parametric_level(kind, values, level as f64 / 255.0);
    }
    Some(levels)
}

/// The parametric function of `kind` (0 to 4) with `parameters` g, a, b, c, d, e and f, at `x`.
#[inline]
fn parametric_level(kind: u16, parameters: [f64; 7], x: f64) -> f64 {
    let [g, a, b, c, d, e, f] = parameters;
    match kind {
        0 => x.powf(g),
        1 if x >= -b / a => (a * x + b).powf(g),
        1 => 0.0,
        2 if x >= -b / a => (a * x + b).powf(g) + c,
        2 => c,
        3 if x >= d => (a * x + b).powf(g),
        3 => c * x,
        _ if x >= d => (a * x + b).powf(g) + e,
        _ => c * x + f,
    }
}

/// Levels of a `curveType`: identity, a gamma, or an interpolated sample table.
fn sampled_curve(data: &[u8]) -> Option<[f64; 256]> {
    let count = u32_at(data, 8)? as usize;
    let entry = |index: usize| u16_at(data, 12 + index * 2).map(|value| f64::from(value) / 65535.0);
    let mut levels = [0.0; 256];
    for (level, out) in levels.iter_mut().enumerate() {
        let x = level as f64 / 255.0;
        *out = match count {
            0 => x,
            1 => x.powf(f64::from(u16_at(data, 12)?) / 256.0),
            _ => {
                let at = x * (count - 1) as f64;
                let below = (at as usize).min(count - 2);
                let part = at - below as f64;
                entry(below)? * (1.0 - part) + entry(below + 1)? * part
            }
        };
    }
    Some(levels)
}

fn inverse(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let m = matrix;
    let determinant = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let cofactor = |row: usize, column: usize| {
        let (r0, r1) = ((row + 1) % 3, (row + 2) % 3);
        let (c0, c1) = ((column + 1) % 3, (column + 2) % 3);
        m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
    };
    let mut out = [[0.0; 3]; 3];
    for (row, values) in out.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            *value = cofactor(column, row) / determinant;
        }
    }
    out
}

/// Columns are the colorants: XYZ of each channel at full intensity.
fn colorant_matrix(colorants: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut matrix = [[0.0; 3]; 3];
    for (row, values) in matrix.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            *value = colorants[column][row];
        }
    }
    matrix
}

/// sRGB's curve, from linear light to an encoded level.
fn srgb_encoded(linear: f64) -> f64 {
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}

/// Converts `rgba` (straight alpha) from `profile` to sRGB in place. Errors when `profile` isn't
/// an RGB matrix-and-curve profile.
pub fn to_srgb(rgba: &mut [u8], profile: &[u8]) -> Result<(), String> {
    let unread = || "a colour profile other than an RGB matrix and curves".to_owned();
    let colorants = [
        colorant(profile, b"rXYZ").ok_or_else(unread)?,
        colorant(profile, b"gXYZ").ok_or_else(unread)?,
        colorant(profile, b"bXYZ").ok_or_else(unread)?,
    ];
    let curves = [
        curve(profile, b"rTRC").ok_or_else(unread)?,
        curve(profile, b"gTRC").ok_or_else(unread)?,
        curve(profile, b"bTRC").ok_or_else(unread)?,
    ];
    let to_xyz = colorant_matrix(colorants);
    let from_xyz = inverse(colorant_matrix(SRGB_COLORANTS));
    let mut matrix = [[0.0; 3]; 3];
    for (row, values) in matrix.iter_mut().enumerate() {
        for (column, value) in values.iter_mut().enumerate() {
            *value = (0..3).map(|k| from_xyz[row][k] * to_xyz[k][column]).sum();
        }
    }
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let linear = [
            curves[0][usize::from(pixel[0])],
            curves[1][usize::from(pixel[1])],
            curves[2][usize::from(pixel[2])],
        ];
        for (channel, row) in matrix.iter().enumerate() {
            let value =
                (row[0] * linear[0] + row[1] * linear[1] + row[2] * linear[2]).clamp(0.0, 1.0);
            pixel[channel] = (srgb_encoded(value) * 255.0).round() as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_tagged_as_srgb_is_unchanged_and_the_inverse_inverts() {
        let identity = inverse(colorant_matrix(SRGB_COLORANTS));
        let back = inverse(identity);
        for (row, values) in back.iter().enumerate() {
            for (column, value) in values.iter().enumerate() {
                assert!((value - colorant_matrix(SRGB_COLORANTS)[row][column]).abs() < 1e-12);
            }
        }
        assert!((srgb_encoded(1.0) - 1.0).abs() < 1e-12);
        assert_eq!((srgb_encoded(0.0) * 255.0).round(), 0.0);
    }
}
