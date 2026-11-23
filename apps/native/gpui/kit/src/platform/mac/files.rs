//! The share sheet, revealing files in Finder, and the name of a file's default app.

use super::appkit::{Id, file_urls};
use objc2::{class, msg_send};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::path::PathBuf;

/// AppKit's `NSRect`, measured from the view's bottom left.
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

// SAFETY: Core Graphics' `CGRect`, a `CGPoint` then a `CGSize`, each two doubles.
unsafe impl objc2::encode::Encode for Rect {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGRect",
        &[
            objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
            objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
        ],
    );
}

/// `NSRectEdgeMinY`: the sheet opens below its anchor.
const BELOW: usize = 1;

/// Opens the share sheet (`NSSharingServicePicker`) for `paths`, anchored at `at` (window points
/// from the top left).
pub fn share(paths: &[PathBuf], window: &gpui::Window, at: gpui::Bounds<gpui::Pixels>) {
    let view: Id = match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    };
    // GPUI's view isn't flipped; its origin is at the bottom left.
    let height = f64::from(window.viewport_size().height);
    let place = Rect {
        x: f64::from(at.origin.x),
        y: height - f64::from(at.origin.y) - f64::from(at.size.height),
        width: f64::from(at.size.width),
        height: f64::from(at.size.height),
    };
    // SAFETY: the picker is created, shown and released on the main thread; AppKit retains it
    // while the sheet is open.
    unsafe {
        let picker: Id = msg_send![class!(NSSharingServicePicker), alloc];
        let picker: Id = msg_send![picker, initWithItems: file_urls(paths)];
        let _: () = msg_send![
            picker,
            showRelativeToRect: place,
            ofView: view,
            preferredEdge: BELOW
        ];
        let _: () = msg_send![picker, release];
    }
}

/// Selects `paths` in Finder, in as few windows as it needs (`activateFileViewerSelectingURLs:`).
pub fn reveal(paths: &[PathBuf]) {
    // SAFETY: the shared workspace and autoreleased URLs.
    unsafe {
        let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let _: () = msg_send![workspace, activateFileViewerSelectingURLs: file_urls(paths)];
    }
}
