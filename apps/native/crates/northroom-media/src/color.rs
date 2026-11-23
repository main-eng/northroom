//! Colour: what a stream's tags mean, and the numbers that convert between a stream and the
//! working space.
//!
//! The working space is linear light with BT.709 primaries, where 1.0 is SDR reference white.
//! HDR goes above 1.0 and colours outside BT.709 go negative, so a working texture that keeps
//! them is float (`gpu::WorkingFormat`).

/// Luminance of SDR reference white, in nits (BT.2408). PQ carries absolute nits.
const REFERENCE_WHITE_NITS: f64 = 203.0;
const PQ_PEAK_NITS: f64 = 10_000.0;
/// HLG scene light at 75% signal, the level BT.2408 gives reference white.
const HLG_REFERENCE_WHITE: f64 = 0.264_962_56;

/// The weights that turn R'G'B' into Y'CbCr.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl Matrix {
    /// The red and blue luma weights; green's is the remainder of 1.
    fn luma_weights(self) -> (f64, f64) {
        match self {
            Self::Bt601 => (0.299, 0.114),
            Self::Bt709 => (0.2126, 0.0722),
            Self::Bt2020 => (0.2627, 0.0593),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primaries {
    Bt709,
    /// SMPTE C, the primaries of 525-line BT.601.
    Bt601Ntsc,
    /// EBU 3213, the primaries of 625-line BT.601.
    Bt601Pal,
    DisplayP3,
    Bt2020,
}

impl Primaries {
    /// CIE xy of red, green and blue. Every set here uses the D65 white point.
    fn chromaticities(self) -> [(f64, f64); 3] {
        match self {
            Self::Bt709 => [(0.640, 0.330), (0.300, 0.600), (0.150, 0.060)],
            Self::Bt601Ntsc => [(0.630, 0.340), (0.310, 0.595), (0.155, 0.070)],
            Self::Bt601Pal => [(0.640, 0.330), (0.290, 0.600), (0.150, 0.060)],
            Self::DisplayP3 => [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
            Self::Bt2020 => [(0.708, 0.292), (0.170, 0.797), (0.131, 0.046)],
        }
    }
}

/// The curve between stored values and light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    /// The BT.709 curve (shared by BT.601 and SDR BT.2020), or an untagged stream. Values map to
    /// light and back by a plain power of 1.961 instead of BT.709's camera curve. That matches
    /// how Core Video and ColorSync read `kCVImageBufferTransferFunction_ITU_R_709_2`, and how
    /// Chromium handles VideoToolbox video (`TransferID::BT709_APPLE`). The camera curve's
    /// inverse would show a stored 24 as sRGB 40; the power shows it as 25.
    Bt709,
    Srgb,
    /// SMPTE ST 2084.
    Pq,
    /// ARIB STD-B67, as iPhones record.
    Hlg,
}

impl Transfer {
    /// The id the shaders branch on.
    fn shader_id(self) -> f32 {
        match self {
            Self::Bt709 => 0.0,
            Self::Srgb => 1.0,
            Self::Pq => 2.0,
            Self::Hlg => 3.0,
        }
    }

    /// Working-space value of the curve's own 1.0.
    fn white_scale(self) -> f64 {
        match self {
            Self::Bt709 | Self::Srgb => 1.0,
            Self::Pq => PQ_PEAK_NITS / REFERENCE_WHITE_NITS,
            Self::Hlg => 1.0 / HLG_REFERENCE_WHITE,
        }
    }
}

/// Where a chroma sample sits relative to the 2×2 luma samples it covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChromaSiting {
    /// On the left column, between the two rows: the H.264 and HEVC default.
    Left,
    /// In the middle of the four: JPEG and MPEG-1.
    Center,
    /// On the top-left luma sample.
    TopLeft,
}

impl ChromaSiting {
    /// The chroma lookup offset, in chroma texels, that lines each luma position up with its
    /// chroma.
    fn lookup_shift(self) -> (f32, f32) {
        match self {
            Self::Left => (0.25, 0.0),
            Self::Center => (0.0, 0.0),
            Self::TopLeft => (0.25, 0.25),
        }
    }
}

/// How a video stream's stored samples map to colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorDescription {
    pub matrix: Matrix,
    pub primaries: Primaries,
    pub transfer: Transfer,
    /// Whether samples use the whole code range rather than the video range (16–235 at 8 bits).
    pub full_range: bool,
    pub chroma_siting: ChromaSiting,
}

impl ColorDescription {
    pub fn is_hdr(&self) -> bool {
        matches!(self.transfer, Transfer::Pq | Transfer::Hlg)
    }

    /// For reports, as `matrix/primaries/transfer/range`.
    pub fn name(&self) -> String {
        format!(
            "{:?}/{:?}/{:?}/{}",
            self.matrix,
            self.primaries,
            self.transfer,
            if self.full_range { "full" } else { "video" }
        )
        .to_lowercase()
    }
}

/// The colour an export is written in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputColor {
    /// 8-bit BT.709.
    Sdr,
    /// 10-bit BT.2020 with the PQ curve.
    HdrPq,
    /// 10-bit BT.2020 with the HLG curve.
    HdrHlg,
}

impl OutputColor {
    pub fn description(self) -> ColorDescription {
        let (matrix, primaries, transfer) = match self {
            Self::Sdr => (Matrix::Bt709, Primaries::Bt709, Transfer::Bt709),
            Self::HdrPq => (Matrix::Bt2020, Primaries::Bt2020, Transfer::Pq),
            Self::HdrHlg => (Matrix::Bt2020, Primaries::Bt2020, Transfer::Hlg),
        };
        ColorDescription {
            matrix,
            primaries,
            transfer,
            full_range: false,
            // Encoders do not signal siting, so decoders assume this one.
            chroma_siting: ChromaSiting::Left,
        }
    }

    pub fn bit_depth(self) -> u32 {
        match self {
            Self::Sdr => 8,
            Self::HdrPq | Self::HdrHlg => 10,
        }
    }

    /// The output that keeps a source's dynamic range.
    pub fn matching(source: &ColorDescription) -> Self {
        match source.transfer {
            Transfer::Pq => Self::HdrPq,
            Transfer::Hlg => Self::HdrHlg,
            Transfer::Bt709 | Transfer::Srgb => Self::Sdr,
        }
    }
}

type Matrix3 = [[f64; 3]; 3];

fn multiply(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    let mut product = [[0.0; 3]; 3];
    for (row, product_row) in product.iter_mut().enumerate() {
        for (column, cell) in product_row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[row][k] * b[k][column]).sum();
        }
    }
    product
}

fn invert(m: &Matrix3) -> Matrix3 {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let determinant: f64 = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum();
    let mut inverse = [[0.0; 3]; 3];
    for (row, inverse_row) in inverse.iter_mut().enumerate() {
        for (column, cell) in inverse_row.iter_mut().enumerate() {
            *cell = cofactor(column, row) / determinant;
        }
    }
    inverse
}

/// Linear RGB in `primaries` to CIE XYZ, with white (1, 1, 1) landing on D65.
fn rgb_to_xyz(primaries: Primaries) -> Matrix3 {
    const D65: (f64, f64) = (0.3127, 0.3290);
    let xyz = |(x, y): (f64, f64)| [x / y, 1.0, (1.0 - x - y) / y];
    let [red, green, blue] = primaries.chromaticities().map(xyz);
    let unscaled = [
        [red[0], green[0], blue[0]],
        [red[1], green[1], blue[1]],
        [red[2], green[2], blue[2]],
    ];
    let white = xyz(D65);
    let inverse = invert(&unscaled);
    let scale: [f64; 3] =
        std::array::from_fn(|row| (0..3).map(|k| inverse[row][k] * white[k]).sum());
    let mut matrix = unscaled;
    for row in &mut matrix {
        for (column, cell) in row.iter_mut().enumerate() {
            *cell *= scale[column];
        }
    }
    matrix
}

/// Linear RGB in `from` primaries to linear RGB in `to` primaries.
fn convert_primaries(from: Primaries, to: Primaries) -> Matrix3 {
    multiply(&invert(&rgb_to_xyz(to)), &rgb_to_xyz(from))
}

/// How stored code values sit in a normalised texture sample.
struct SampleRange {
    luma_offset: f64,
    luma_span: f64,
    chroma_offset: f64,
    chroma_span: f64,
}

impl SampleRange {
    /// 8-bit samples fill their byte; deeper samples sit in the high bits of a 16-bit word.
    fn new(bit_depth: u32, full_range: bool) -> Self {
        let word_bits = if bit_depth == 8 { 8 } else { 16 };
        // One code value, as a normalised sample.
        let code = f64::from(1u32 << (word_bits - bit_depth)) / f64::from((1u32 << word_bits) - 1);
        let depth_scale = f64::from(1u32 << (bit_depth - 8));
        let largest_code = f64::from((1u32 << bit_depth) - 1);
        let (black, luma_span, chroma_span) = if full_range {
            (0.0, largest_code, largest_code)
        } else {
            (16.0 * depth_scale, 219.0 * depth_scale, 224.0 * depth_scale)
        };
        Self {
            luma_offset: black * code,
            luma_span: luma_span * code,
            chroma_offset: 128.0 * depth_scale * code,
            chroma_span: chroma_span * code,
        }
    }
}

/// Floats in each transform's uniform block.
pub(crate) const TRANSFORM_UNIFORM_FLOATS: usize = 24;

/// The uniform block both transform shaders share (see `TRANSFORM_UNIFORM` in `gpu`).
fn pack_uniform(
    luma: (f64, f64),
    chroma: (f64, f64),
    color: &ColorDescription,
    white_scale: f64,
    primaries: &Matrix3,
) -> [f32; TRANSFORM_UNIFORM_FLOATS] {
    let (red_weight, blue_weight) = color.matrix.luma_weights();
    let (shift_x, shift_y) = color.chroma_siting.lookup_shift();
    let mut block = [0.0; TRANSFORM_UNIFORM_FLOATS];
    block[0] = luma.0 as f32;
    block[1] = luma.1 as f32;
    block[2] = chroma.0 as f32;
    block[3] = chroma.1 as f32;
    block[4] = red_weight as f32;
    block[5] = blue_weight as f32;
    block[6] = shift_x;
    block[7] = shift_y;
    block[8] = color.transfer.shader_id();
    block[9] = white_scale as f32;
    // A 3×3 matrix is three columns, each padded to four floats.
    for column in 0..3 {
        for row in 0..3 {
            block[12 + column * 4 + row] = primaries[row][column] as f32;
        }
    }
    block
}

/// Uniforms that convert stored Y'CbCr samples of `color` to the working space.
pub(crate) fn input_transform_uniform(
    color: &ColorDescription,
    bit_depth: u32,
) -> [f32; TRANSFORM_UNIFORM_FLOATS] {
    let range = SampleRange::new(bit_depth, color.full_range);
    pack_uniform(
        (range.luma_offset, 1.0 / range.luma_span),
        (range.chroma_offset, 1.0 / range.chroma_span),
        color,
        color.transfer.white_scale(),
        &convert_primaries(color.primaries, Primaries::Bt709),
    )
}

/// Uniforms that convert the working space to stored Y'CbCr samples of `color`.
pub(crate) fn output_transform_uniform(
    color: &ColorDescription,
    bit_depth: u32,
) -> [f32; TRANSFORM_UNIFORM_FLOATS] {
    let range = SampleRange::new(bit_depth, color.full_range);
    pack_uniform(
        (range.luma_offset, range.luma_span),
        (range.chroma_offset, range.chroma_span),
        color,
        1.0 / color.transfer.white_scale(),
        &convert_primaries(Primaries::Bt709, color.primaries),
    )
}

/// Converts stored Y'CbCr samples to the working space on the CPU, matching the GPU input shader
/// (`gpu`). Used where no GPU draws the frame, such as a video still off the Mac. Samples are
/// normalised as a texture reads them, 0..1 of their word.
pub struct SampleConverter {
    luma: (f64, f64),
    chroma: (f64, f64),
    luma_weights: (f64, f64),
    transfer: Transfer,
    white_scale: f64,
    primaries: Matrix3,
}

impl SampleConverter {
    pub fn new(color: &ColorDescription, bit_depth: u32) -> Self {
        let range = SampleRange::new(bit_depth, color.full_range);
        Self {
            luma: (range.luma_offset, 1.0 / range.luma_span),
            chroma: (range.chroma_offset, 1.0 / range.chroma_span),
            luma_weights: color.matrix.luma_weights(),
            transfer: color.transfer,
            white_scale: color.transfer.white_scale(),
            primaries: convert_primaries(color.primaries, Primaries::Bt709),
        }
    }

    /// Linear light in BT.709 primaries, where 1.0 is SDR reference white.
    pub fn working(&self, luma: f64, cb: f64, cr: f64) -> [f64; 3] {
        let luma = (luma - self.luma.0) * self.luma.1;
        let (cb, cr) = (
            (cb - self.chroma.0) * self.chroma.1,
            (cr - self.chroma.0) * self.chroma.1,
        );
        let (red_weight, blue_weight) = self.luma_weights;
        let red = luma + 2.0 * (1.0 - red_weight) * cr;
        let blue = luma + 2.0 * (1.0 - blue_weight) * cb;
        let green =
            (luma - red_weight * red - blue_weight * blue) / (1.0 - red_weight - blue_weight);
        let light =
            [red, green, blue].map(|value| self.transfer.to_linear(value) * self.white_scale);
        std::array::from_fn(|row| (0..3).map(|k| self.primaries[row][k] * light[k]).sum())
    }
}

impl Transfer {
    /// Linear light for a stored value, matching the shaders' `to_linear`.
    fn to_linear(self, value: f64) -> f64 {
        match self {
            // Odd, so colours outside the gamut (negative) survive.
            Self::Bt709 => value.signum() * value.abs().powf(1.961),
            Self::Srgb => value.signum() * srgb_to_linear(value.abs()),
            Self::Pq => {
                let power = value.max(0.0).powf(1.0 / 78.843_75);
                let ratio = (power - 0.835_937_5).max(0.0) / (18.851_562_5 - 18.6875 * power);
                ratio.powf(1.0 / 0.159_301_757_812_5)
            }
            Self::Hlg => {
                let signal = value.max(0.0);
                if signal <= 0.5 {
                    signal * signal / 3.0
                } else {
                    (((signal - 0.559_910_73) / 0.178_832_77).exp() + 0.284_668_92) / 12.0
                }
            }
        }
    }
}

fn srgb_to_linear(value: f64) -> f64 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

/// Encodes linear light (0..1) as an sRGB value (0..1).
pub fn linear_to_srgb(light: f64) -> f64 {
    if light <= 0.003_130_8 {
        light * 12.92
    } else {
        1.055 * light.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_matrix_close(actual: &Matrix3, expected: Matrix3, tolerance: f64) {
        for row in 0..3 {
            for column in 0..3 {
                let difference = (actual[row][column] - expected[row][column]).abs();
                assert!(
                    difference <= tolerance,
                    "[{row}][{column}]: {} is not within {tolerance} of {}",
                    actual[row][column],
                    expected[row][column],
                );
            }
        }
    }

    const IDENTITY: Matrix3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    #[test]
    fn bt2020_to_bt709_matches_the_matrix_in_bt2087() {
        assert_matrix_close(
            &convert_primaries(Primaries::Bt2020, Primaries::Bt709),
            [
                [1.6605, -0.5876, -0.0728],
                [-0.1246, 1.1329, -0.0083],
                [-0.0182, -0.1006, 1.1187],
            ],
            0.0005,
        );
    }

    #[test]
    fn display_p3_to_bt709_matches_the_published_matrix() {
        assert_matrix_close(
            &convert_primaries(Primaries::DisplayP3, Primaries::Bt709),
            [
                [1.2249, -0.2247, 0.0],
                [-0.0420, 1.0419, 0.0],
                [-0.0197, -0.0786, 1.0979],
            ],
            0.0005,
        );
    }

    #[test]
    fn converting_primaries_there_and_back_changes_nothing() {
        for primaries in [
            Primaries::Bt601Ntsc,
            Primaries::Bt601Pal,
            Primaries::DisplayP3,
            Primaries::Bt2020,
        ] {
            let there = convert_primaries(primaries, Primaries::Bt709);
            let back = convert_primaries(Primaries::Bt709, primaries);
            assert_matrix_close(&multiply(&back, &there), IDENTITY, 1e-12);
        }
        assert_matrix_close(
            &convert_primaries(Primaries::Bt709, Primaries::Bt709),
            IDENTITY,
            1e-12,
        );
    }

    #[test]
    fn video_range_puts_black_and_white_on_their_codes() {
        let eight_bit = SampleRange::new(8, false);
        assert!((eight_bit.luma_offset - 16.0 / 255.0).abs() < 1e-12);
        assert!((eight_bit.luma_offset + eight_bit.luma_span - 235.0 / 255.0).abs() < 1e-12);
        assert!((eight_bit.chroma_offset - 128.0 / 255.0).abs() < 1e-12);
        // Ten-bit codes sit in the high bits of a 16-bit word.
        let ten_bit = SampleRange::new(10, false);
        assert!((ten_bit.luma_offset - 64.0 * 64.0 / 65535.0).abs() < 1e-12);
        assert!((ten_bit.luma_offset + ten_bit.luma_span - 940.0 * 64.0 / 65535.0).abs() < 1e-12);
        let full = SampleRange::new(8, true);
        assert_eq!((full.luma_offset, full.luma_span), (0.0, 1.0));
    }

    #[test]
    fn hlg_reference_white_is_the_curve_at_three_quarters_signal() {
        let light = (((0.75_f64 - 0.559_910_73) / 0.178_832_77).exp() + 0.284_668_92) / 12.0;
        assert!((light - HLG_REFERENCE_WHITE).abs() < 1e-6);
    }

    #[test]
    fn input_and_output_blocks_undo_each_other() {
        let color = OutputColor::HdrPq.description();
        let input = input_transform_uniform(&color, 10);
        let output = output_transform_uniform(&color, 10);
        // Luma and chroma scales, and the white scale, are reciprocals.
        for slot in [1, 3, 9] {
            assert!(
                (input[slot] * output[slot] - 1.0).abs() < 1e-5,
                "slot {slot}"
            );
        }
        // Offsets, luma weights and the curve are shared.
        for slot in [0, 2, 4, 5, 8] {
            assert_eq!(input[slot], output[slot], "slot {slot}");
        }
    }

    #[test]
    fn the_processor_converts_samples_as_the_shader_does() {
        let sdr = OutputColor::Sdr.description();
        let converter = SampleConverter::new(&sdr, 8);
        // Video-range black and white, and a grey whose light the Mac's curve gives.
        let black = converter.working(16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        let white = converter.working(235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        for channel in 0..3 {
            assert!(black[channel].abs() < 1e-9);
            assert!((white[channel] - 1.0).abs() < 1e-9);
        }
        let grey = converter.working((16.0 + 219.0 * 0.5) / 255.0, 128.0 / 255.0, 128.0 / 255.0);
        assert!((grey[0] - 0.5_f64.powf(1.961)).abs() < 1e-9);
        // Full red in BT.709: R' is 1 and G', B' are 0.
        let (red_weight, blue_weight) = (0.2126, 0.0722);
        let red = converter.working(
            (16.0 + 219.0 * red_weight) / 255.0,
            (128.0 - 224.0 * red_weight / (2.0 * (1.0 - blue_weight))) / 255.0,
            (128.0 + 224.0 * 0.5) / 255.0,
        );
        assert!(
            (red[0] - 1.0).abs() < 1e-9 && red[1].abs() < 1e-9 && red[2].abs() < 1e-9,
            "{red:?}"
        );
        assert!((linear_to_srgb(srgb_to_linear(0.5)) - 0.5).abs() < 1e-12);
        assert!((linear_to_srgb(1.0) - 1.0).abs() < 1e-12);
    }
}
