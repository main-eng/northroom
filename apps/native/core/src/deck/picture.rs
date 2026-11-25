//! Pictures in formats the system's decoders don't read everywhere, drawn as a deck of one page
//! holding the picture (ADR-0015):
//!
//! - a Photoshop file (`.psd`, `.psb`) as the flattened image it saves for readers without its
//!   layers, decoded here and written as a PNG;
//! - a camera raw photo (`.dng`, `.cr2`, `.cr3`, `.nef`, `.arw`, `.raf`, `.orf`, `.rw2`) as the
//!   largest JPEG preview it holds. Every camera writes one, full size or close to it, for its
//!   own screen; developing the raw sensor data is left to photo editors.
//!
//! Left out: a raw photo's orientation (its preview is drawn as stored), Photoshop's Lab and
//! multichannel modes, and 32-bit images.

use super::{read_bytes, Colour, Deck, Fit, Item, Placed, ReadError, Result, Slide, Source};
use crate::geometry::Rect;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Crc};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

/// The most pixels a decoded picture may have, so a damaged size can't ask for all memory.
const PIXELS_MAX: u64 = 200_000_000;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

/// A page that shows only `picture`, its size in pixels taken as points.
fn deck_of(picture: Source, width: u32, height: u32) -> Deck {
    let (width, height) = (f64::from(width.max(1)), f64::from(height.max(1)));
    Deck {
        width,
        height,
        fit: Fit::Picture,
        slides: vec![Slide {
            ground: Colour {
                alpha: 0.0,
                ..Colour::WHITE
            },
            items: vec![Item::Picture(Placed {
                frame: Rect::new(0.0, 0.0, width, height),
                picture: 0,
                crop: [0.0; 4],
            })],
        }],
        pictures: vec![picture],
    }
}

// --- PNG -------------------------------------------------------------------------------------

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

/// Eight-bit RGBA pixels, rows top to bottom, as a PNG.
pub fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    // Eight bits, RGBA, deflate, no filter method beyond the standard, no interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let row = width as usize * 4;
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    for line in rgba.chunks(row) {
        // Each row starts with its filter: none. Writing to memory can't fail.
        let _ = encoder.write_all(&[0]);
        let _ = encoder.write_all(line);
    }
    let compressed = encoder.finish().unwrap_or_default();
    let mut out = PNG_SIGNATURE.to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    out
}

// --- Photoshop -----------------------------------------------------------------------------

fn damaged<T>(why: &str) -> Result<T> {
    Err(ReadError::Damaged(why.to_owned()))
}

/// A cursor over big-endian data.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(count)
            .filter(|end| *end <= self.bytes.len());
        let Some(end) = end else {
            return damaged("The Photoshop file ends early.");
        };
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }

    /// Skips a section that starts with its length, four bytes or (in a large document) eight.
    fn skip_section(&mut self, wide: bool) -> Result<()> {
        let length = if wide {
            self.u64()?
        } else {
            u64::from(self.u32()?)
        };
        self.take(usize::try_from(length).unwrap_or(usize::MAX))?;
        Ok(())
    }
}

/// A Photoshop document's colour mode, for those drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Grey,
    Indexed,
    Rgb,
    Cmyk,
}

impl Mode {
    fn of(code: u16) -> Result<Self> {
        Ok(match code {
            1 => Mode::Grey,
            2 => Mode::Indexed,
            3 => Mode::Rgb,
            4 => Mode::Cmyk,
            _ => return damaged("The Photoshop file's colour mode is not drawn."),
        })
    }

    /// How many channels hold its colour, before any alpha.
    fn colour_channels(self) -> usize {
        match self {
            Mode::Grey | Mode::Indexed => 1,
            Mode::Rgb => 3,
            Mode::Cmyk => 4,
        }
    }
}

/// Undoes PackBits run-length coding into exactly `length` bytes.
fn unpack_bits(packed: &[u8], length: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(length);
    let mut at = 0;
    while at < packed.len() && out.len() < length {
        let header = packed[at] as i8;
        at += 1;
        if header >= 0 {
            let count = header as usize + 1;
            let end = (at + count).min(packed.len());
            out.extend_from_slice(&packed[at..end]);
            at = end;
        } else if header != -128 {
            let count = 1 - header as isize;
            if let Some(&value) = packed.get(at) {
                out.extend(std::iter::repeat_n(value, count as usize));
            }
            at += 1;
        }
    }
    out.resize(length, 0);
    out
}

/// The planes of the flattened image, each `width × height` bytes at eight bits: the first
/// `count` channels, the high byte of each sample in a 16-bit document.
fn read_planes(
    reader: &mut Reader,
    channels: usize,
    count: usize,
    size: (usize, usize),
    depth: u16,
    wide: bool,
) -> Result<Vec<Vec<u8>>> {
    let (width, height) = size;
    let sample = usize::from(depth / 8).max(1);
    let row = width * sample;
    let compression = reader.u16()?;
    let rows: Vec<Vec<u8>> = match compression {
        0 => (0..channels * height)
            .map(|_| reader.take(row).map(<[u8]>::to_vec))
            .collect::<Result<_>>()?,
        1 => {
            let mut lengths = Vec::with_capacity(channels * height);
            for _ in 0..channels * height {
                lengths.push(if wide {
                    reader.u32()? as usize
                } else {
                    usize::from(reader.u16()?)
                });
            }
            lengths
                .into_iter()
                .map(|length| reader.take(length).map(|packed| unpack_bits(packed, row)))
                .collect::<Result<_>>()?
        }
        _ => return damaged("The Photoshop file's image is compressed in a way not read."),
    };
    let planes = rows
        .chunks(height)
        .take(count)
        .map(|plane| {
            plane
                .iter()
                .flat_map(|line| line.iter().step_by(sample).copied())
                .collect()
        })
        .collect();
    Ok(planes)
}

/// The planes as RGBA, in `mode` with `palette` for an indexed one.
fn to_rgba(planes: &[Vec<u8>], mode: Mode, palette: &[u8], alpha: bool) -> Vec<u8> {
    let pixels = planes[0].len();
    let mut rgba = Vec::with_capacity(pixels * 4);
    let alpha_plane = alpha.then(|| &planes[mode.colour_channels()]);
    for at in 0..pixels {
        let [red, green, blue] = match mode {
            Mode::Grey => [planes[0][at]; 3],
            Mode::Rgb => [planes[0][at], planes[1][at], planes[2][at]],
            Mode::Indexed => {
                let index = usize::from(planes[0][at]);
                let channel = |plane: usize| palette.get(plane * 256 + index).copied().unwrap_or(0);
                [channel(0), channel(1), channel(2)]
            }
            // Photoshop stores ink inverted: 255 is none. Each colour is its ink less the black.
            Mode::Cmyk => {
                let black = u16::from(planes[3][at]);
                [0, 1, 2].map(|plane| ((u16::from(planes[plane][at]) * black) / 255) as u8)
            }
        };
        let opacity = alpha_plane.map_or(255, |plane| plane[at]);
        rgba.extend_from_slice(&[red, green, blue, opacity]);
    }
    rgba
}

/// Reads a Photoshop document's flattened image as a PNG and its size in pixels.
pub fn read_photoshop(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(4)? != b"8BPS" {
        return Err(ReadError::NotItsKind);
    }
    let wide = match reader.u16()? {
        1 => false,
        2 => true,
        _ => return Err(ReadError::NotItsKind),
    };
    reader.take(6)?;
    let channels = usize::from(reader.u16()?);
    let (height, width) = (reader.u32()?, reader.u32()?);
    let depth = reader.u16()?;
    let mode = Mode::of(reader.u16()?)?;
    if !matches!(depth, 8 | 16) || channels < mode.colour_channels() {
        return damaged("The Photoshop file's depth is not drawn.");
    }
    if u64::from(width) * u64::from(height) > PIXELS_MAX || width == 0 || height == 0 {
        return damaged("The Photoshop file's size can't be drawn.");
    }
    let palette_length = reader.u32()? as usize;
    let palette = reader.take(palette_length)?;
    reader.skip_section(false)?;
    reader.skip_section(wide)?;
    let alpha = mode != Mode::Indexed && channels > mode.colour_channels();
    let count = mode.colour_channels() + usize::from(alpha);
    let size = (width as usize, height as usize);
    let planes = read_planes(&mut reader, channels, count, size, depth, wide)?;
    let rgba = to_rgba(&planes, mode, palette, alpha);
    Ok((encode_png(width, height, &rgba), width, height))
}

// --- Raw photos ----------------------------------------------------------------------------

/// A JPEG found in a file: where it is and how many pixels it has.
struct Found {
    start: usize,
    end: usize,
    size: (u32, u32),
}

/// The JPEG starting at `start` (at its `FF D8`), if it is whole: its markers in order up to
/// its scan, then entropy-coded data up to its end marker. Its size is from its frame header.
fn jpeg_at(bytes: &[u8], start: usize) -> Option<Found> {
    let be16 = |at: usize| {
        Some(usize::from(u16::from_be_bytes(
            bytes.get(at..at + 2)?.try_into().ok()?,
        )))
    };
    let mut at = start + 2;
    let mut size = None;
    loop {
        if *bytes.get(at)? != 0xff {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            0xff => at += 1,
            0xd9 => {
                return size.map(|size| Found {
                    start,
                    end: at + 2,
                    size,
                })
            }
            0x01 | 0xd0..=0xd7 => at += 2,
            0xda => {
                at = at + 2 + be16(at + 2)?;
                at = scan_entropy(bytes, at)?;
            }
            _ => {
                let length = be16(at + 2)?;
                let frame = matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc);
                if frame {
                    let height = u32::try_from(be16(at + 5)?).ok()?;
                    let width = u32::try_from(be16(at + 7)?).ok()?;
                    size = Some((width, height));
                }
                at = at + 2 + length;
            }
        }
    }
}

/// Where entropy-coded data from `at` ends: the first marker that is not a stuffed byte or a
/// restart.
fn scan_entropy(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let found = bytes.get(at..)?.iter().position(|byte| *byte == 0xff)?;
        at += found;
        match *bytes.get(at + 1)? {
            0x00 | 0xd0..=0xd7 | 0xff => at += 1,
            _ => return Some(at),
        }
    }
}

/// The largest whole JPEG in `bytes`, by pixels: a raw photo's preview.
fn largest_jpeg(bytes: &[u8]) -> Option<Found> {
    let mut best: Option<Found> = None;
    let mut at = 0;
    while let Some(offset) = bytes.get(at..).and_then(|rest| {
        rest.windows(3)
            .position(|window| window == [0xff, 0xd8, 0xff])
    }) {
        let start = at + offset;
        let Some(found) = jpeg_at(bytes, start) else {
            at = start + 1;
            continue;
        };
        at = found.end;
        let pixels = |found: &Found| u64::from(found.size.0) * u64::from(found.size.1);
        if best
            .as_ref()
            .is_none_or(|best| pixels(&found) > pixels(best))
        {
            best = Some(found);
        }
    }
    best
}

/// Reads a raw photo's largest preview as a JPEG and its size in pixels.
pub fn read_raw(bytes: &[u8]) -> Result<(Vec<u8>, u32, u32)> {
    let Some(found) = largest_jpeg(bytes) else {
        return damaged("The raw photo holds no preview.");
    };
    Ok((
        bytes[found.start..found.end].to_vec(),
        found.size.0,
        found.size.1,
    ))
}

/// Reads a Photoshop file as a deck of its picture.
pub fn read_photoshop_file(path: &Path) -> Result<Deck> {
    let (png, width, height) = read_photoshop(&read_bytes(path)?)?;
    let picture = Source::Bytes {
        bytes: Arc::new(png),
        extension: "png",
    };
    Ok(deck_of(picture, width, height))
}

/// Reads a raw photo as a deck of its preview.
pub fn read_raw_file(path: &Path) -> Result<Deck> {
    let (jpeg, width, height) = read_raw(&read_bytes(path)?)?;
    let picture = Source::Bytes {
        bytes: Arc::new(jpeg),
        extension: "jpeg",
    };
    Ok(deck_of(picture, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_runs_unpack() {
        // Three literal bytes, then a run of four sevens, then a no-op.
        assert_eq!(
            unpack_bits(&[2, 1, 2, 3, 0xfd, 7, 0x80], 7),
            [1, 2, 3, 7, 7, 7, 7]
        );
        assert_eq!(unpack_bits(&[0], 3), [0, 0, 0]);
    }

    #[test]
    fn a_png_has_its_header_and_size() {
        let png = encode_png(2, 1, &[255, 0, 0, 255, 0, 255, 0, 128]);
        assert!(png.starts_with(PNG_SIGNATURE));
        assert_eq!(super::super::flow::picture_pixels(&png), Some((2, 1)));
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]));
    }

    /// A Photoshop document of 2 by 1 pixels in `mode` with its planes stored raw.
    fn document(mode: u16, channels: u16, planes: &[&[u8]]) -> Vec<u8> {
        let mut bytes = b"8BPS\x00\x01\x00\x00\x00\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&channels.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&8u16.to_be_bytes());
        bytes.extend_from_slice(&mode.to_be_bytes());
        bytes.extend_from_slice(&[0; 12]);
        bytes.extend_from_slice(&0u16.to_be_bytes());
        for plane in planes {
            bytes.extend_from_slice(plane);
        }
        bytes
    }

    #[test]
    fn a_photoshop_image_reads_in_each_colour_mode() {
        let decode = |bytes: &[u8]| {
            let (png, width, height) = read_photoshop(bytes).expect("read");
            assert_eq!((width, height), (2, 1));
            png
        };
        let rgb = document(3, 4, &[&[255, 0], &[0, 255], &[0, 0], &[255, 128]]);
        assert!(decode(&rgb).starts_with(PNG_SIGNATURE));
        let planes = vec![vec![255, 0], vec![0, 255], vec![0, 0], vec![255, 128]];
        assert_eq!(
            to_rgba(&planes, Mode::Rgb, &[], true),
            [255, 0, 0, 255, 0, 255, 0, 128]
        );
        // No ink at all is white; full black ink is black.
        let cmyk = vec![vec![255, 255], vec![255, 255], vec![255, 255], vec![255, 0]];
        assert_eq!(
            to_rgba(&cmyk, Mode::Cmyk, &[], false),
            [255, 255, 255, 255, 0, 0, 0, 255]
        );
        assert!(read_photoshop(b"8BPX").is_err());
        assert!(read_photoshop(&document(9, 3, &[])).is_err());
    }

    #[test]
    fn the_largest_whole_jpeg_is_a_raw_photos_preview() {
        let jpeg = |width: u8, height: u8| {
            vec![
                0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0, height, 0, width, 1, 1, 0x11, 0, 0xff, 0xda,
                0, 8, 1, 1, 0, 0, 0x3f, 0, 0x12, 0xff, 0x00, 0x34, 0xff, 0xd0, 0x56, 0xff, 0xd9,
            ]
        };
        let mut file = b"II*\x00junk".to_vec();
        file.extend(jpeg(4, 3));
        file.extend_from_slice(&[0xff, 0xd8, 0xff, 0x00]);
        let large = jpeg(40, 30);
        file.extend(&large);
        file.extend(jpeg(8, 6));
        let (preview, width, height) = read_raw(&file).expect("a preview");
        assert_eq!((preview, width, height), (large, 40, 30));
        assert!(read_raw(b"no pictures here").is_err());
    }
}
