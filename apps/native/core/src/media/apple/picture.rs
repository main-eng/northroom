//! Picture size as shown, read from the header by Image I/O. Orientations 5 to 8 swap width
//! and height.

use super::av::{number_of, CFDictionaryGetValue, CFTypeRef, Cf};
use std::ptr;

#[allow(non_upper_case_globals)]
#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    static kCGImagePropertyPixelWidth: CFTypeRef;
    static kCGImagePropertyPixelHeight: CFTypeRef;
    static kCGImagePropertyOrientation: CFTypeRef;
    fn CGImageSourceCreateWithURL(url: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
}

/// The size in pixels the picture at `path` is shown at; `None` if Image I/O can't read it or
/// it has no size.
pub fn picture_dimensions(path: &str) -> Option<(u32, u32)> {
    let url = Cf::file_url(path).ok()?;
    // SAFETY: `url` is a live URL; each object created is owned by a `Cf`, and the properties
    // read are borrowed from the dictionary while it is held.
    unsafe {
        let source = Cf::created(
            CGImageSourceCreateWithURL(url.0, ptr::null()),
            "picture source",
        )
        .ok()?;
        let properties = Cf::created(
            CGImageSourceCopyPropertiesAtIndex(source.0, 0, ptr::null()),
            "picture properties",
        )
        .ok()?;
        let read = |key: CFTypeRef| number_of(CFDictionaryGetValue(properties.0, key));
        let (width, height) = (
            read(kCGImagePropertyPixelWidth)?,
            read(kCGImagePropertyPixelHeight)?,
        );
        if width < 1.0 || height < 1.0 {
            return None;
        }
        let turned = read(kCGImagePropertyOrientation).unwrap_or(1.0) >= 5.0;
        let (width, height) = (width as u32, height as u32);
        Some(if turned {
            (height, width)
        } else {
            (width, height)
        })
    }
}
