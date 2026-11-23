//! The colour space the window is drawn in.
//!
//! AppKit draws a window into a backing store in the display's colour space (Display P3 on most
//! Macs) and blends there. GPUI's layer has no colour space unless asked, so its sRGB bytes reach
//! a wide-gamut display unconverted and look too saturated. On a Mac the GPUI window is therefore
//! a Display P3 surface (`WindowOptions::use_display_p3`, `DISPLAY_P3`), and Core Graphics
//! converts every sRGB colour and picture to Display P3 before GPUI draws it (`shown`, `image`).
//! Colours and blends then match AppKit's bytes, and wide-gamut pictures keep their colours.
//! Windows and Linux present sRGB, so nothing is converted there.
//!
//! Window captures are in the same space and are saved tagged with it (`save`).

use gpui::{Hsla, RenderImage, Rgba};
use std::sync::Arc;

/// Whether the window is a Display P3 surface (macOS) rather than sRGB.
pub const DISPLAY_P3: bool = cfg!(target_os = "macos");

/// The sRGB `color` converted to the window's colour space.
pub fn shown(color: Rgba) -> Hsla {
    #[cfg(target_os = "macos")]
    let [r, g, b] = mac::color([color.r, color.g, color.b]);
    #[cfg(not(target_os = "macos"))]
    let [r, g, b] = [color.r, color.g, color.b];
    Rgba {
        r,
        g,
        b,
        a: color.a,
    }
    .into()
}

/// Converts a `width` × `height` sRGB picture to the window's colour space in place. Each pixel
/// is `step` bytes, with red, green and blue at the offsets `at`.
pub fn convert(pixels: &mut [u8], width: u32, height: u32, step: usize, at: [usize; 3]) {
    #[cfg(target_os = "macos")]
    mac::pixels(pixels, width, height, step, at);
    #[cfg(not(target_os = "macos"))]
    let _ = (pixels, width, height, step, at);
}

/// A GPUI image for this window from straight-alpha sRGB BGRA pixels.
pub fn image(width: u32, height: u32, mut bgra: Vec<u8>) -> Option<Arc<RenderImage>> {
    if bgra.len() != width as usize * height as usize * 4 {
        return None;
    }
    convert(&mut bgra, width, height, 4, [2, 1, 0]);
    image::ImageBuffer::from_raw(width, height, bgra)
        .map(|buffer| Arc::new(RenderImage::new([image::Frame::new(buffer)])))
}

/// Saves a window capture (`render_to_image`, in the window's colour space) as a PNG tagged with
/// that colour space.
pub fn save(picture: &image::RgbaImage, path: &str) -> Result<(), String> {
    use image::ImageEncoder;
    let file = std::fs::File::create(path).map_err(|error| error.to_string())?;
    let encoder = image::codecs::png::PngEncoder::new(std::io::BufWriter::new(file));
    #[cfg(target_os = "macos")]
    let encoder = {
        let mut encoder = encoder;
        encoder
            .set_icc_profile(mac::profile())
            .map_err(|error| error.to_string())?;
        encoder
    };
    encoder
        .write_image(
            picture.as_raw(),
            picture.width(),
            picture.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
mod mac {
    //! Core Graphics' sRGB to Display P3 conversions: colours as it converts a fill colour,
    //! pictures as it draws an sRGB image into a Display P3 bitmap.

    use std::ffi::c_void;

    type Ref = *const c_void;

    #[repr(C)]
    struct Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGColorSpaceCreateWithName(name: Ref) -> Ref;
        fn CGColorSpaceCopyICCData(space: Ref) -> Ref;
        fn CGColorCreateSRGB(red: f64, green: f64, blue: f64, alpha: f64) -> Ref;
        fn CGColorCreateCopyByMatchingToColorSpace(
            space: Ref,
            intent: i32,
            color: Ref,
            options: Ref,
        ) -> Ref;
        fn CGColorGetComponents(color: Ref) -> *const f64;
        fn CGDataProviderCreateWithData(
            info: *mut c_void,
            data: *const c_void,
            size: usize,
            release: Ref,
        ) -> Ref;
        fn CGImageCreate(
            width: usize,
            height: usize,
            bits: usize,
            bits_per_pixel: usize,
            row: usize,
            space: Ref,
            info: u32,
            provider: Ref,
            decode: *const f64,
            interpolate: bool,
            intent: i32,
        ) -> Ref;
        fn CGBitmapContextCreate(
            data: *mut c_void,
            width: usize,
            height: usize,
            bits: usize,
            row: usize,
            space: Ref,
            info: u32,
        ) -> Ref;
        fn CGContextDrawImage(context: Ref, rect: Rect, image: Ref);
        static kCGColorSpaceSRGB: Ref;
        static kCGColorSpaceDisplayP3: Ref;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFDataGetLength(data: Ref) -> isize;
        fn CFDataGetBytePtr(data: Ref) -> *const u8;
    }

    /// Red, green and blue bytes with an unused fourth (`kCGImageAlphaNoneSkipLast`).
    const RGBX: u32 = 5;
    const DEFAULT_INTENT: i32 = 0;

    fn display_p3() -> Ref {
        // SAFETY: returns a named colour space that the caller releases.
        unsafe { CGColorSpaceCreateWithName(kCGColorSpaceDisplayP3) }
    }

    /// Display P3's colour profile.
    pub fn profile() -> Vec<u8> {
        // SAFETY: the colour space and its profile data are both released here.
        unsafe {
            let space = display_p3();
            let data = CGColorSpaceCopyICCData(space);
            CFRelease(space);
            let bytes =
                std::slice::from_raw_parts(CFDataGetBytePtr(data), CFDataGetLength(data) as usize)
                    .to_vec();
            CFRelease(data);
            bytes
        }
    }

    pub fn color(rgb: [f32; 3]) -> [f32; 3] {
        let [red, green, blue] = rgb.map(f64::from);
        // SAFETY: the colours and colour space are created and released here. A colour matched
        // to an RGB space has four components.
        unsafe {
            let space = display_p3();
            let srgb = CGColorCreateSRGB(red, green, blue, 1.0);
            let matched = CGColorCreateCopyByMatchingToColorSpace(
                space,
                DEFAULT_INTENT,
                srgb,
                std::ptr::null(),
            );
            let components = std::slice::from_raw_parts(CGColorGetComponents(matched), 3);
            let shown = [0, 1, 2].map(|channel| components[channel] as f32);
            for made in [matched, srgb, space] {
                CFRelease(made);
            }
            shown
        }
    }

    pub fn pixels(pixels: &mut [u8], width: u32, height: u32, step: usize, at: [usize; 3]) {
        let (width, height) = (width as usize, height as usize);
        if width == 0 || height == 0 {
            return;
        }
        let from: Vec<u8> = pixels
            .chunks_exact(step)
            .flat_map(|pixel| [pixel[at[0]], pixel[at[1]], pixel[at[2]], 255])
            .collect();
        let mut to = vec![0u8; from.len()];
        // SAFETY: the image reads `from` and the bitmap writes `to`, both width × height × 4
        // bytes and alive until both are released here.
        unsafe {
            let (srgb, space) = (CGColorSpaceCreateWithName(kCGColorSpaceSRGB), display_p3());
            let provider = CGDataProviderCreateWithData(
                std::ptr::null_mut(),
                from.as_ptr().cast(),
                from.len(),
                std::ptr::null(),
            );
            let image = CGImageCreate(
                width,
                height,
                8,
                32,
                width * 4,
                srgb,
                RGBX,
                provider,
                std::ptr::null(),
                false,
                DEFAULT_INTENT,
            );
            let context = CGBitmapContextCreate(
                to.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space,
                RGBX,
            );
            let all = Rect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            };
            CGContextDrawImage(context, all, image);
            for made in [context, image, provider, space, srgb] {
                CFRelease(made);
            }
        }
        for (pixel, shown) in pixels.chunks_exact_mut(step).zip(to.as_chunks::<4>().0) {
            for channel in 0..3 {
                pixel[at[channel]] = shown[channel];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_pictures_land_where_core_graphics_puts_them() {
        let bytes = |rgb: [u8; 3]| {
            let mut pixel = rgb.to_vec();
            convert(&mut pixel, 1, 1, 3, [0, 1, 2]);
            pixel
        };
        let filled = |rgb: [u8; 3]| {
            let [r, g, b] = rgb.map(|channel| f32::from(channel) / 255.0);
            let shown: Rgba = shown(Rgba { r, g, b, a: 1.0 }).into();
            [shown.r, shown.g, shown.b].map(|channel| (channel * 255.0).round() as u8)
        };
        if !DISPLAY_P3 {
            assert_eq!(bytes([229, 57, 53]), [229, 57, 53]);
            assert_eq!(filled([229, 57, 53]), [229, 57, 53]);
            return;
        }
        // Greys are unchanged; colours match Core Graphics' Display P3 output.
        for grey in [0, 17, 128, 238, 250, 255] {
            assert_eq!(bytes([grey; 3]), [grey; 3]);
            assert_eq!(filled([grey; 3]), [grey; 3]);
        }
        assert_eq!(bytes([225, 45, 45]), [207, 63, 55]);
        assert_eq!(bytes([240, 195, 60]), [233, 197, 87]);
        assert_eq!(filled([225, 45, 45]), [207, 63, 55]);
        // The red in `scripts/drive-media/gradient.png`.
        assert_eq!(bytes([229, 57, 53]), [211, 72, 62]);
    }
}
