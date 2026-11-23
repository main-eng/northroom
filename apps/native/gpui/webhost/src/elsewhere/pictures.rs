//! Picture encoding off the Mac, using the `image` crate GPUI already depends on. Writes PNG and
//! reads any format the crate supports into premultiplied RGBA, top row first.

use crate::picture::Picture;
use image::{ImageFormat, RgbaImage};

/// Converts straight alpha to premultiplied in place.
fn premultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        if alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
            }
        }
    }
}

/// Converts premultiplied alpha to straight in place.
fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = u16::from(pixel[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut pixel[..3] {
                *channel = ((u16::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// Encodes the picture as PNG.
pub fn png(picture: &Picture) -> Option<Vec<u8>> {
    let mut rgba = picture.rgba.clone();
    unpremultiply(&mut rgba);
    let image = RgbaImage::from_raw(picture.width, picture.height, rgba)?;
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, ImageFormat::Png).ok()?;
    Some(bytes.into_inner())
}

/// Decodes a PNG, or any format the crate reads, into the host's format.
pub fn decode(bytes: &[u8]) -> Option<Picture> {
    let image = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (width, height) = image.dimensions();
    let mut rgba = image.into_raw();
    premultiply(&mut rgba);
    Some(Picture {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_comes_back_from_its_png_as_it_went_in() {
        let picture = Picture {
            width: 2,
            height: 1,
            // Opaque red, then half-transparent blue (premultiplied).
            rgba: vec![255, 0, 0, 255, 0, 0, 128, 128],
        };
        let back = decode(&png(&picture).expect("a PNG")).expect("a picture");
        assert_eq!((back.width, back.height), (2, 1));
        assert_eq!(&back.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(back.rgba[7], 128);
        assert!((i16::from(back.rgba[6]) - 128).abs() <= 1);
    }
}
