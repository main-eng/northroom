//! A PDF image's pixels: its samples at 1 to 16 bits a component in its colour space, its
//! decode array, a stencil mask painted in the fill colour, a JPEG's pixels, and a soft mask or
//! mask as its alpha.
//!
//! A JPEG is decoded here only when it can't be left to the system (`render::Interpreter::
//! placement`): when it is see-through, masked, turned or clipped to a shape.
//!
//! Left out: CCITT fax and JBIG2 images (common in scanned black-and-white pages), colour-key
//! masks, and decoding JPEG 2000, which the system draws without transparency or a shaped clip.

use super::colour::Space;
use super::filter;
use super::object::{Dict, Document, Object, Stream};
use super::raster::Rgba;

/// The most pixels an image is decoded with, so a damaged size can't ask for all memory.
const PIXELS_MAX: usize = 64_000_000;

/// An image's pixels as straight RGBA.
pub struct Picture {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Reads samples of `bits` each from packed rows.
struct Samples<'a> {
    data: &'a [u8],
    bits: usize,
    row_bytes: usize,
}

impl Samples<'_> {
    fn get(&self, row: usize, index: usize) -> f32 {
        let start = row * self.row_bytes;
        let bit = index * self.bits;
        let byte = |at: usize| u32::from(*self.data.get(start + at).unwrap_or(&0));
        let value = match self.bits {
            8 => byte(bit / 8),
            16 => byte(bit / 8) << 8 | byte(bit / 8 + 1),
            _ => (byte(bit / 8) >> (8 - self.bits - bit % 8)) & ((1 << self.bits) - 1),
        };
        value as f32 / ((1u32 << self.bits) - 1) as f32
    }
}

fn number(document: &Document, dict: &Dict, long: &str, short: &str) -> Option<i64> {
    document
        .entry(dict, long)
        .as_i64()
        .or_else(|| document.entry(dict, short).as_i64())
}

fn decode_array(document: &Document, dict: &Dict) -> Vec<f32> {
    let object = match dict.get("Decode").or_else(|| dict.get("D")) {
        Some(object) => document.resolve(object),
        None => Object::Null,
    };
    object
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| document.resolve(item).as_f64())
                .map(|value| value as f32)
                .collect()
        })
        .unwrap_or_default()
}

/// An image's pixels as RGBA, its soft mask or mask applied. `fill` paints a stencil mask. A
/// JPEG is decoded here; a JPEG 2000 is not read.
pub fn read(
    document: &Document,
    stream: &Stream,
    fill: Rgba,
    resources: Option<&Dict>,
) -> Option<Picture> {
    let mut picture = match filter::codec(stream, Some(document)) {
        Some("jpeg") => decode_jpeg(&filter::decode(stream, Some(document)).ok()?)?,
        Some(_) => return None,
        None => read_samples(document, stream, fill, resources)?,
    };
    apply_masks(document, &stream.dict, &mut picture);
    Some(picture)
}

/// A JPEG's pixels.
fn decode_jpeg(bytes: &[u8]) -> Option<Picture> {
    use zune_jpeg::zune_core::{
        bytestream::ZCursor, colorspace::ColorSpace, options::DecoderOptions,
    };
    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options);
    let rgba = decoder.decode().ok()?;
    let info = decoder.info()?;
    let (width, height) = (usize::from(info.width), usize::from(info.height));
    (width * height * 4 == rgba.len() && width * height <= PIXELS_MAX).then_some(Picture {
        rgba,
        width,
        height,
    })
}

/// An image's own samples: a stencil painted in `fill`, or colours in its colour space.
fn read_samples(
    document: &Document,
    stream: &Stream,
    fill: Rgba,
    resources: Option<&Dict>,
) -> Option<Picture> {
    let dict = &stream.dict;
    let width = number(document, dict, "Width", "W")?.max(0) as usize;
    let height = number(document, dict, "Height", "H")?.max(0) as usize;
    if width == 0 || height == 0 || width.saturating_mul(height) > PIXELS_MAX {
        return None;
    }
    let data = filter::decode(stream, Some(document)).ok()?;
    let decode = decode_array(document, dict);
    if is_stencil(document, dict) {
        return Some(stencil(
            &data,
            width,
            height,
            fill,
            decode.first() == Some(&1.0),
        ));
    }
    let space_object = dict
        .get("ColorSpace")
        .or_else(|| dict.get("CS"))
        .cloned()
        .unwrap_or(Object::Name("DeviceGray".into()));
    let space = named_space(document, &space_object, resources).unwrap_or(Space::Gray);
    let bits = number(document, dict, "BitsPerComponent", "BPC")
        .unwrap_or(8)
        .clamp(1, 16) as usize;
    let rgba = colour_pixels(&data, (width, height), &space, bits, &decode);
    Some(Picture {
        rgba,
        width,
        height,
    })
}

fn is_stencil(document: &Document, dict: &Dict) -> bool {
    matches!(document.entry(dict, "ImageMask"), Object::Bool(true))
        || matches!(document.entry(dict, "IM"), Object::Bool(true))
}

/// A colour space named in resources (`/CS0`), or given directly.
pub fn named_space(
    document: &Document,
    object: &Object,
    resources: Option<&Dict>,
) -> Option<Space> {
    let object = document.resolve(object);
    if let Some(name) = object.as_name() {
        let found = resources
            .map(|resources| document.entry(resources, "ColorSpace"))
            .and_then(|spaces| {
                spaces
                    .as_dict()
                    .and_then(|spaces| spaces.get(name))
                    .cloned()
            });
        if let Some(found) = found {
            return Space::read(document, &found, 0);
        }
    }
    Space::read(document, &object, 0)
}

fn stencil(data: &[u8], width: usize, height: usize, fill: Rgba, inverted: bool) -> Picture {
    let samples = Samples {
        data,
        bits: 1,
        row_bytes: width.div_ceil(8),
    };
    let colour = fill.map(|channel| (channel.clamp(0.0, 1.0) * 255.0) as u8);
    let mut rgba = Vec::with_capacity(width * height * 4);
    for row in 0..height {
        for column in 0..width {
            let painted = (samples.get(row, column) < 0.5) != inverted;
            let alpha = if painted { colour[3] } else { 0 };
            rgba.extend_from_slice(&[colour[0], colour[1], colour[2], alpha]);
        }
    }
    Picture {
        rgba,
        width,
        height,
    }
}

fn colour_pixels(
    data: &[u8],
    size: (usize, usize),
    space: &Space,
    bits: usize,
    decode: &[f32],
) -> Vec<u8> {
    let (width, height) = size;
    let components = space.components();
    let samples = Samples {
        data,
        bits,
        row_bytes: (width * components * bits).div_ceil(8),
    };
    let indexed = matches!(space, Space::Indexed { .. });
    let mut rgba = Vec::with_capacity(width * height * 4);
    let mut values = vec![0.0f32; components];
    for row in 0..height {
        for column in 0..width {
            for (component, value) in values.iter_mut().enumerate() {
                let raw = samples.get(row, column * components + component);
                let (low, high) = match decode.get(component * 2..component * 2 + 2) {
                    Some(range) => (range[0], range[1]),
                    None if indexed => (0.0, ((1u32 << bits) - 1) as f32),
                    None => (0.0, 1.0),
                };
                *value = low + raw * (high - low);
            }
            let [r, g, b] = space.rgb(&values);
            rgba.extend_from_slice(&[(r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8, 255]);
        }
    }
    rgba
}

/// Applies an image's soft mask (its gray is the alpha) or its mask (a stencil whose marked
/// samples hide the image).
fn apply_masks(document: &Document, dict: &Dict, picture: &mut Picture) {
    if let Object::Stream(soft) = document.entry(dict, "SMask") {
        if let Some(gray) = mask_gray(document, &soft) {
            apply_alpha(picture, &gray, |value| value);
        }
    } else if let Object::Stream(mask) = document.entry(dict, "Mask") {
        apply_stencil_mask(document, &mask, picture);
    }
}

/// A mask's marked samples hide the image, or show it when its `Decode` is `[1 0]`.
fn apply_stencil_mask(document: &Document, mask: &Stream, picture: &mut Picture) {
    let Some(gray) = mask_gray(document, mask) else {
        return;
    };
    if decode_array(document, &mask.dict).first() == Some(&1.0) {
        apply_alpha(picture, &gray, |value| value);
    } else {
        apply_alpha(picture, &gray, |value| 255 - value);
    }
}

/// A mask's samples as gray, 0 to 255: its own samples, or a JPEG's first channel.
fn mask_gray(document: &Document, mask: &Stream) -> Option<Picture> {
    let dict = &mask.dict;
    match filter::codec(mask, Some(document)) {
        Some("jpeg") => return decode_jpeg(&filter::decode(mask, Some(document)).ok()?),
        Some(_) => return None,
        None => {}
    }
    let width = number(document, dict, "Width", "W")?.max(1) as usize;
    let height = number(document, dict, "Height", "H")?.max(1) as usize;
    if width.saturating_mul(height) > PIXELS_MAX {
        return None;
    }
    let data = filter::decode(mask, Some(document)).ok()?;
    let bits = if is_stencil(document, dict) {
        1
    } else {
        number(document, dict, "BitsPerComponent", "BPC")
            .unwrap_or(8)
            .clamp(1, 16) as usize
    };
    let samples = Samples {
        data: &data,
        bits,
        row_bytes: (width * bits).div_ceil(8),
    };
    let rgba = (0..height)
        .flat_map(|row| (0..width).map(move |column| (row, column)))
        .flat_map(|(row, column)| [(samples.get(row, column) * 255.0).round() as u8, 0, 0, 255])
        .collect();
    Some(Picture {
        rgba,
        width,
        height,
    })
}

/// Sets each pixel's alpha to `alpha_of` the mask's gray there, the mask stretched to the
/// image's size.
fn apply_alpha(picture: &mut Picture, mask: &Picture, alpha_of: impl Fn(u8) -> u8) {
    let (width, height) = (picture.width, picture.height);
    for row in 0..height {
        for column in 0..width {
            let at = (row * mask.height / height * mask.width + column * mask.width / width) * 4;
            let alpha = alpha_of(mask.rgba[at]);
            let pixel = &mut picture.rgba[(row * width + column) * 4 + 3];
            *pixel = (u16::from(*pixel) * u16::from(alpha) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_unpack_at_every_depth() {
        let bits = Samples {
            data: &[0b1010_0000],
            bits: 1,
            row_bytes: 1,
        };
        assert_eq!((bits.get(0, 0), bits.get(0, 1)), (1.0, 0.0));
        let nibbles = Samples {
            data: &[0xf0],
            bits: 4,
            row_bytes: 1,
        };
        assert_eq!((nibbles.get(0, 0), nibbles.get(0, 1)), (1.0, 0.0));
        let wide = Samples {
            data: &[0xff, 0xff],
            bits: 16,
            row_bytes: 2,
        };
        assert_eq!(wide.get(0, 0), 1.0);
        let pixels = colour_pixels(&[255, 0, 0, 0, 0, 255], (2, 1), &Space::Rgb, 8, &[]);
        assert_eq!(pixels, [255, 0, 0, 255, 0, 0, 255, 255]);
        let rgba = stencil(&[0b0100_0000], 2, 1, [1.0, 0.0, 0.0, 1.0], false).rgba;
        assert_eq!(rgba, [255, 0, 0, 255, 255, 0, 0, 0]);
    }
}
