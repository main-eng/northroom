//! Quick Look thumbnails (`QLThumbnailGenerator`), the same ones Finder shows. Only content
//! thumbnails are requested, never icons, so files without one keep Northroom's glyph.

use super::appkit::{Id, Size, ns_string};
use super::{Pixels, pixels_of};
use block2::RcBlock;
use objc2::{class, msg_send};

#[link(name = "QuickLookThumbnailing", kind = "framework")]
unsafe extern "C" {}

/// `QLThumbnailGenerationRequestRepresentationTypeThumbnail`: the file's contents, not its icon
/// or a low-quality placeholder.
const THUMBNAIL_ONLY: usize = 1 << 2;

/// Opaque Core Graphics image, used only behind a pointer.
#[repr(C)]
struct CGImage {
    _opaque: [u8; 0],
}

// SAFETY: a `CGImageRef` is a pointer to the opaque `struct CGImage`.
unsafe impl objc2::encode::RefEncode for CGImage {
    const ENCODING_REF: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGImage", &[]));
}

/// Quick Look's thumbnail of `path`, at most `pixels` on the long side, in the window's colour
/// space; none when there is no content thumbnail. Blocks on Quick Look, which renders
/// out of process, so call it off the main thread.
pub fn system_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    let (tell, told) = std::sync::mpsc::channel();
    // Quick Look calls back once on its own queue, with the thumbnail or with an error.
    let answer = RcBlock::new(move |representation: Id, _error: Id| {
        let drawn = (!representation.is_null())
            .then(|| {
                // SAFETY: the representation keeps its image alive while the handler runs.
                unsafe {
                    let image: *const CGImage = msg_send![representation, CGImage];
                    (!image.is_null())
                        .then(|| pixels_of(image.cast()))
                        .flatten()
                }
            })
            .flatten();
        // The receiver is gone only if it stopped waiting.
        let _ = tell.send(drawn);
    });
    let size = Size {
        width: f64::from(pixels),
        height: f64::from(pixels),
    };
    // SAFETY: the shared generator, and a request released here after the callback; the URL and
    // path are autoreleased.
    let request: Id = unsafe {
        let url: Id = msg_send![class!(NSURL), fileURLWithPath: ns_string(path)];
        let request: Id = msg_send![class!(QLThumbnailGenerationRequest), alloc];
        let request: Id = msg_send![
            request,
            initWithFileAtURL: url,
            size: size,
            scale: 1.0f64,
            representationTypes: THUMBNAIL_ONLY
        ];
        let generator: Id = msg_send![class!(QLThumbnailGenerator), sharedGenerator];
        let _: () = msg_send![
            generator,
            generateBestRepresentationForRequest: request,
            completionHandler: &*answer
        ];
        request
    };
    let drawn = told.recv().ok().flatten();
    // SAFETY: releases the single reference from `init`.
    unsafe {
        let _: () = msg_send![request, release];
    }
    drawn
}
