//! The colour picker's eyedropper (`NSColorSampler`). Returns the picked colour in sRGB, or
//! none when the loupe is dismissed.

use super::appkit::Id;
use block2::RcBlock;
use objc2::{class, msg_send};
use std::cell::Cell;

/// Shows the loupe and calls `picked` with red, green and blue (0 to 1), or none.
pub fn sample_colour(picked: impl FnOnce(Option<[f64; 3]>) + 'static) {
    let picked = Cell::new(Some(picked));
    let handler = RcBlock::new(move |color: Id| {
        let Some(picked) = picked.take() else {
            return;
        };
        if color.is_null() {
            picked(None);
            return;
        }
        // SAFETY: AppKit's picked colour, read on the main thread.
        let channels = unsafe {
            let space: Id = msg_send![class!(NSColorSpace), sRGBColorSpace];
            let srgb: Id = msg_send![color, colorUsingColorSpace: space];
            (!srgb.is_null()).then(|| {
                let red: f64 = msg_send![srgb, redComponent];
                let green: f64 = msg_send![srgb, greenComponent];
                let blue: f64 = msg_send![srgb, blueComponent];
                [red, green, blue]
            })
        };
        picked(channels);
    });
    // SAFETY: the sampler retains itself and the handler while it shows.
    unsafe {
        let sampler: Id = msg_send![class!(NSColorSampler), new];
        let _: () = msg_send![sampler, showSamplerWithSelectionHandler: &*handler];
        let _: () = msg_send![sampler, release];
    }
}
