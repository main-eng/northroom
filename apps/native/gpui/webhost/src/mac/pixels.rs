//! Picture encoding through Core Graphics and Image I/O. Every picture is drawn into a
//! premultiplied sRGB RGBA bitmap at its exact size, so the bytes do not depend on the display.

use crate::picture::Picture;
use objc2_core_foundation::{CFData, CFMutableData, CFRetained, CFString, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextGetData, CGBitmapInfo, CGColorRenderingIntent,
    CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo, kCGColorSpaceSRGB,
};
use objc2_image_io::{CGImageDestination, CGImageSource};

pub fn srgb() -> CFRetained<CGColorSpace> {
    // SAFETY: a constant of the framework.
    CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
        .expect("the sRGB colour space exists")
}

pub fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
    CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    }
}

/// An sRGB bitmap of premultiplied RGBA, `width` × `height`, rows of `width` × 4 bytes.
pub fn context(width: u32, height: u32) -> Option<CFRetained<CGContext>> {
    // SAFETY: Core Graphics allocates the bitmap itself (null data).
    unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            width as usize,
            height as usize,
            8,
            width as usize * 4,
            Some(&srgb()),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
}

/// Copies out the pixels of a bitmap from `context`.
pub fn bytes_of(context: &CGContext, width: u32, height: u32) -> Option<Vec<u8>> {
    let data = CGBitmapContextGetData(Some(context));
    if data.is_null() {
        return None;
    }
    // SAFETY: the bitmap holds `width` × `height` × 4 bytes.
    Some(
        unsafe {
            std::slice::from_raw_parts(data.cast::<u8>(), width as usize * height as usize * 4)
        }
        .to_vec(),
    )
}

/// Reads `image` into a picture at its own size.
pub fn picture_of(image: &CGImage) -> Option<Picture> {
    let (width, height) = (
        CGImage::width(Some(image)) as u32,
        CGImage::height(Some(image)) as u32,
    );
    let context = context(width, height)?;
    CGContext::draw_image(
        Some(&context),
        rect(0.0, 0.0, f64::from(width), f64::from(height)),
        Some(image),
    );
    Some(Picture {
        width,
        height,
        rgba: bytes_of(&context, width, height)?,
    })
}

/// Wraps the picture as a Core Graphics image.
pub fn image_of(picture: &Picture) -> Option<CFRetained<CGImage>> {
    let data = CFData::from_bytes(&picture.rgba);
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: the provider holds width × height × 4 bytes of premultiplied RGBA.
    unsafe {
        CGImage::new(
            picture.width as usize,
            picture.height as usize,
            8,
            32,
            picture.width as usize * 4,
            Some(&srgb()),
            CGBitmapInfo(CGImageAlphaInfo::PremultipliedLast.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

pub fn png(picture: &Picture) -> Option<Vec<u8>> {
    let image = image_of(picture)?;
    let data = CFMutableData::new(None, 0)?;
    // SAFETY: a PNG destination of one image, no options.
    unsafe {
        let destination = CGImageDestination::with_data(
            &data,
            &CFString::from_static_str("public.png"),
            1,
            None,
        )?;
        destination.add_image(&image, None);
        if !destination.finalize() {
            return None;
        }
    }
    Some(data.to_vec())
}

pub fn decode(bytes: &[u8]) -> Option<Picture> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: no options.
    let image = unsafe { CGImageSource::with_data(&data, None)?.image_at_index(0, None)? };
    picture_of(&image)
}
