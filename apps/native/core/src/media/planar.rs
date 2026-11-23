//! Video stills where frames are planes in memory (Windows and Linux). The decoder's frame is
//! box-filtered down to the still's size, converted to sRGB with the same numbers the GPU uses
//! (`northroom_media::color::SampleConverter`), and turned as the stream is shown. The Mac uses
//! its own decoder's stills (`apple::still`).

use super::picture_size;
use northroom_media::color::{linear_to_srgb, ColorDescription, SampleConverter};
use northroom_media::pixel_buffer::PixelLayout;

/// A frame as a picture: sRGB, one byte per channel, in BGRA order. Video frames are opaque; a
/// picture file's alpha is kept straight, not premultiplied (`windows::picture`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Still {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// How a stream's pictures are turned for display: quarter turns clockwise, then a horizontal
/// mirror.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Turn {
    pub(super) quarter_turns: u8,
    pub(super) mirrored: bool,
}

impl Turn {
    /// The turn for a clockwise rotation in degrees, or `None` if it isn't a quarter turn.
    pub(super) fn clockwise(degrees: u32, mirrored: bool) -> Option<Self> {
        degrees.is_multiple_of(90).then_some(Self {
            quarter_turns: (degrees / 90 % 4) as u8,
            mirrored,
        })
    }

    /// The turn for an EXIF orientation (1 to 8). Other values are upright, as readers treat an
    /// unreadable orientation.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub(super) fn of_orientation(orientation: u16) -> Self {
        let (quarter_turns, mirrored) = match orientation {
            2 => (0, true),
            3 => (2, false),
            4 => (2, true),
            5 => (1, true),
            6 => (1, false),
            7 => (3, true),
            8 => (3, false),
            _ => (0, false),
        };
        Self {
            quarter_turns,
            mirrored,
        }
    }

    /// A `width` by `height` picture's size once turned.
    pub(super) fn shown_size(self, width: u32, height: u32) -> (u32, u32) {
        if self.quarter_turns % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        }
    }

    pub(super) fn apply(self, still: Still) -> Still {
        if self == Self::default() {
            return still;
        }
        let (width, height) = (still.width as usize, still.height as usize);
        let (shown_width, shown_height) = self.shown_size(still.width, still.height);
        let mut bgra = Vec::with_capacity(still.bgra.len());
        for y in 0..shown_height as usize {
            for x in 0..shown_width as usize {
                let (source_x, source_y) =
                    self.source_pixel(x, y, (width, height), shown_width as usize);
                let at = (source_y * width + source_x) * 4;
                bgra.extend_from_slice(&still.bgra[at..at + 4]);
            }
        }
        Still {
            width: shown_width,
            height: shown_height,
            bgra,
        }
    }

    /// The stored pixel of a `size` picture that shown pixel `x`, `y` comes from, in a picture
    /// `shown_width` wide once turned.
    #[inline]
    fn source_pixel(
        self,
        x: usize,
        y: usize,
        (width, height): (usize, usize),
        shown_width: usize,
    ) -> (usize, usize) {
        let x = if self.mirrored {
            shown_width - 1 - x
        } else {
            x
        };
        match self.quarter_turns {
            1 => (y, height - 1 - x),
            2 => (width - 1 - x, height - 1 - y),
            3 => (width - 1 - y, x),
            _ => (x, y),
        }
    }
}

/// A frame's two planes, each in rows of its own width with no padding.
pub(super) struct Planes<'a> {
    pub(super) layout: PixelLayout,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) luma: &'a [u8],
    pub(super) chroma: &'a [u8],
}

impl Planes<'_> {
    /// Sample `index` of `plane` as a texture reads it: 0..1 of its byte or word.
    fn sample(&self, plane: &[u8], index: usize) -> f64 {
        match self.layout {
            PixelLayout::Nv12 { .. } => f64::from(plane[index]) / 255.0,
            PixelLayout::P010 { .. } => {
                f64::from(u16::from_le_bytes([plane[index * 2], plane[index * 2 + 1]])) / 65535.0
            }
        }
    }

    /// The picture of these planes, no larger than `max_pixels` on either side.
    fn picture(&self, color: &ColorDescription, max_pixels: u32) -> Still {
        let (width, height) = picture_size(self.width, self.height, max_pixels);
        let converter = SampleConverter::new(color, self.layout.bit_depth());
        let columns = spans(self.width, width);
        let rows = spans(self.height, height);
        let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
        for &(top, bottom) in &rows {
            for &(left, right) in &columns {
                let luma = self.mean_luma((top, bottom), (left, right));
                let (cb, cr) = self.mean_chroma(chroma_span(top, bottom), chroma_span(left, right));
                let light = converter.working(luma, cb, cr);
                let [red, green, blue] = light
                    .map(|value| (linear_to_srgb(value.clamp(0.0, 1.0)) * 255.0).round() as u8);
                bgra.extend_from_slice(&[blue, green, red, 255]);
            }
        }
        Still {
            width,
            height,
            bgra,
        }
    }

    /// The mean of the luma samples in `rows` and `columns` (each a start and an end).
    #[inline]
    fn mean_luma(&self, (top, bottom): (usize, usize), (left, right): (usize, usize)) -> f64 {
        let mut luma = 0.0;
        for y in top..bottom {
            luma += (left..right)
                .map(|x| self.sample(self.luma, y * self.width as usize + x))
                .sum::<f64>();
        }
        luma / ((bottom - top) * (right - left)) as f64
    }

    /// The means of the Cb and Cr samples in chroma `rows` and `columns`.
    #[inline]
    fn mean_chroma(
        &self,
        (top, bottom): (usize, usize),
        (left, right): (usize, usize),
    ) -> (f64, f64) {
        let chroma_width = self.width.div_ceil(2) as usize;
        let (mut cb, mut cr) = (0.0, 0.0);
        for y in top..bottom {
            for x in left..right {
                let at = (y * chroma_width + x) * 2;
                cb += self.sample(self.chroma, at);
                cr += self.sample(self.chroma, at + 1);
            }
        }
        let count = ((bottom - top) * (right - left)) as f64;
        (cb / count, cr / count)
    }
}

/// The run of `source` samples each of `target` pixels covers, at least one each.
fn spans(source: u32, target: u32) -> Vec<(usize, usize)> {
    let (source, target) = (source as usize, target as usize);
    (0..target)
        .map(|index| {
            let start = index * source / target;
            (start, ((index + 1) * source / target).max(start + 1))
        })
        .collect()
}

/// The chroma samples under luma samples `start..end`, at least one.
fn chroma_span(start: usize, end: usize) -> (usize, usize) {
    let first = start / 2;
    (first, end.div_ceil(2).max(first + 1))
}

/// The frame of `reader` shown at `seconds`, as a picture no larger than `max_pixels` on either
/// side, turned by `turn`.
#[cfg(not(target_os = "macos"))]
pub(super) fn still_of(
    reader: &mut impl super::VideoSource,
    turn: Turn,
    seconds: f64,
    max_pixels: u32,
) -> super::MediaResult<Still> {
    let frame = reader.frame_at(seconds)?.buffer;
    let layout = frame
        .layout()
        .ok_or_else(|| super::MediaError("the frame has no planes".into()))?;
    let (width, height) = frame.size();
    let planes = Planes {
        layout,
        width,
        height,
        luma: frame.plane(0),
        chroma: frame.plane(1),
    };
    Ok(turn.apply(planes.picture(&reader.info().color, max_pixels.max(1))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use northroom_media::color::{Matrix, Primaries, Transfer};

    const BT709: ColorDescription = ColorDescription {
        matrix: Matrix::Bt709,
        primaries: Primaries::Bt709,
        transfer: Transfer::Bt709,
        full_range: false,
        chroma_siting: northroom_media::color::ChromaSiting::Left,
    };

    /// A `width` by `height` 8-bit frame of one grey level `luma`, neutral chroma.
    fn grey(width: u32, height: u32, luma: u8) -> (Vec<u8>, Vec<u8>) {
        let chroma = (width.div_ceil(2) * height.div_ceil(2) * 2) as usize;
        (vec![luma; (width * height) as usize], vec![128; chroma])
    }

    #[test]
    fn video_range_black_and_white_are_black_and_white() {
        for (luma, shown) in [(16, 0), (235, 255)] {
            let (luma, chroma) = grey(4, 2, luma);
            let planes = Planes {
                layout: PixelLayout::Nv12 { full_range: false },
                width: 4,
                height: 2,
                luma: &luma,
                chroma: &chroma,
            };
            let still = planes.picture(&BT709, 64);
            assert_eq!((still.width, still.height), (4, 2));
            assert!(
                still
                    .bgra
                    .chunks(4)
                    .all(|pixel| pixel == [shown, shown, shown, 255]),
                "{:?}",
                &still.bgra[..4]
            );
        }
    }

    #[test]
    fn a_grey_is_shown_as_the_mac_shows_it() {
        // The BT.709 curve is read as a plain power of 1.961 (`Transfer::Bt709`), then written
        // as sRGB; full range here, so the stored value is the curve's own.
        let (luma, chroma) = grey(2, 2, 128);
        let full = ColorDescription {
            full_range: true,
            ..BT709
        };
        let planes = Planes {
            layout: PixelLayout::Nv12 { full_range: true },
            width: 2,
            height: 2,
            luma: &luma,
            chroma: &chroma,
        };
        let light = (128.0_f64 / 255.0).powf(1.961);
        let shown = ((1.055 * light.powf(1.0 / 2.4) - 0.055) * 255.0).round() as u8;
        assert_eq!(shown, 139);
        assert_eq!(
            &planes.picture(&full, 2).bgra[..4],
            &[shown, shown, shown, 255]
        );
    }

    #[test]
    fn ten_bit_samples_sit_in_the_high_bits() {
        // Video-range white at 10 bits is 940, stored as 940 << 6.
        let white = (940_u16 << 6).to_le_bytes();
        let neutral = (512_u16 << 6).to_le_bytes();
        let luma: Vec<u8> = white.repeat(4);
        let chroma: Vec<u8> = neutral.repeat(2);
        let planes = Planes {
            layout: PixelLayout::P010 { full_range: false },
            width: 2,
            height: 2,
            luma: &luma,
            chroma: &chroma,
        };
        assert_eq!(&planes.picture(&BT709, 2).bgra[..4], &[255, 255, 255, 255]);
    }

    #[test]
    fn a_large_frame_is_shrunk_by_averaging() {
        // Left half black, right half white, 8 x 4 into 4 x 2: each picture pixel covers two
        // stored columns, all of one side.
        let mut luma = Vec::new();
        for _ in 0..4 {
            luma.extend([16, 16, 16, 16, 235, 235, 235, 235]);
        }
        let chroma = vec![128; 4 * 2 * 2];
        let planes = Planes {
            layout: PixelLayout::Nv12 { full_range: false },
            width: 8,
            height: 4,
            luma: &luma,
            chroma: &chroma,
        };
        let still = planes.picture(&BT709, 4);
        assert_eq!((still.width, still.height), (4, 2));
        let row: Vec<u8> = still.bgra[..16].chunks(4).map(|pixel| pixel[0]).collect();
        assert_eq!(row, [0, 0, 255, 255]);
        // Each picture pixel averaging one black and one white column would be grey: not here.
        assert_eq!(spans(8, 4), [(0, 2), (2, 4), (4, 6), (6, 8)]);
        assert_eq!(spans(3, 2), [(0, 1), (1, 3)]);
        assert_eq!(chroma_span(1, 3), (0, 2));
    }

    #[test]
    fn red_is_red() {
        let (red_weight, blue_weight) = (0.2126, 0.0722);
        let y = (16.0_f64 + 219.0 * red_weight).round() as u8;
        let cb = (128.0_f64 - 224.0 * red_weight / (2.0 * (1.0 - blue_weight))).round() as u8;
        let cr = (128.0 + 112.0_f64).round() as u8;
        let (luma, chroma) = (vec![y; 4], vec![cb, cr]);
        let planes = Planes {
            layout: PixelLayout::Nv12 { full_range: false },
            width: 2,
            height: 2,
            luma: &luma,
            chroma: &chroma,
        };
        let pixel = &planes.picture(&BT709, 2).bgra[..4];
        assert!(pixel[0] < 8 && pixel[1] < 8 && pixel[2] > 250, "{pixel:?}");
    }

    /// A 3 x 2 picture whose pixels are numbered 0 to 5 in their blue byte, row by row.
    fn numbered() -> Still {
        Still {
            width: 3,
            height: 2,
            bgra: (0..6).flat_map(|n| [n, 0, 0, 255]).collect(),
        }
    }

    fn blues(still: &Still) -> Vec<u8> {
        still.bgra.chunks(4).map(|pixel| pixel[0]).collect()
    }

    #[test]
    fn pictures_turn_as_their_streams_say() {
        // 0 1 2      clockwise:  3 0      half:  5 4 3     anticlockwise:  2 5
        // 3 4 5                  4 1             2 1 0                     1 4
        //                        5 2                                       0 3
        let turned = |degrees, mirrored| {
            Turn::clockwise(degrees, mirrored)
                .expect("quarter turn")
                .apply(numbered())
        };
        let quarter = turned(90, false);
        assert_eq!(
            ((quarter.width, quarter.height), blues(&quarter)),
            ((2, 3), vec![3, 0, 4, 1, 5, 2])
        );
        assert_eq!(blues(&turned(180, false)), [5, 4, 3, 2, 1, 0]);
        assert_eq!(blues(&turned(270, false)), [2, 5, 1, 4, 0, 3]);
        assert_eq!(blues(&turned(0, true)), [2, 1, 0, 5, 4, 3]);
        // Turned, then mirrored.
        assert_eq!(blues(&turned(90, true)), [0, 3, 1, 4, 2, 5]);
        assert_eq!(turned(0, false), numbered());
        assert_eq!(Turn::clockwise(45, false), None);
        assert_eq!(
            Turn::clockwise(450, false).map(|turn| turn.shown_size(30, 20)),
            Some((20, 30))
        );
    }

    #[test]
    fn pictures_turn_as_their_exif_orientation_says() {
        let oriented = |orientation| blues(&Turn::of_orientation(orientation).apply(numbered()));
        assert_eq!(oriented(1), [0, 1, 2, 3, 4, 5]);
        assert_eq!(oriented(2), [2, 1, 0, 5, 4, 3], "mirrored");
        assert_eq!(oriented(3), [5, 4, 3, 2, 1, 0], "upside down");
        assert_eq!(oriented(4), [3, 4, 5, 0, 1, 2], "flipped top to bottom");
        assert_eq!(oriented(5), [0, 3, 1, 4, 2, 5], "transposed");
        assert_eq!(oriented(6), [3, 0, 4, 1, 5, 2], "turned clockwise");
        assert_eq!(oriented(7), [5, 2, 4, 1, 3, 0], "transversed");
        assert_eq!(oriented(8), [2, 5, 1, 4, 0, 3], "turned anticlockwise");
        assert_eq!(oriented(0), oriented(1), "unknown is upright");
        assert_eq!(Turn::of_orientation(6).shown_size(30, 20), (20, 30));
    }
}
