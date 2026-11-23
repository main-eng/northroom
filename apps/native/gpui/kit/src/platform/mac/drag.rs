//! Dragging files out of the window. Starts an AppKit dragging session from GPUI's view with the
//! current mouse event. Each item's pasteboard writer is the file's `NSURL`, so any app that
//! accepts files gets real files, and its image is the window's thumbnail or the file icon.
//! The session offers only copy, so Finder never moves files out of the drive.

use super::appkit::{Id, application, ns_string};
use crate::lists::chip::Carried;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{MainThreadMarker, MainThreadOnly, class, define_class, msg_send};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;
use std::sync::Arc;

/// `NSEventTypeLeftMouseDown` and `NSEventTypeLeftMouseDragged`, the events a dragging session
/// can start from.
const LEFT_MOUSE_DOWN: usize = 1;
const LEFT_MOUSE_DRAGGED: usize = 6;

/// `NSDragOperationCopy`.
const COPY: usize = 1;

/// Side of the square each drag image fits in, in points. Matches Finder's icon size.
const PICTURE_POINTS: f64 = 64.0;

/// `kCGImageAlphaFirst` with `kCGBitmapByteOrder32Little`: BGRA bytes with straight alpha, as
/// GPUI stores images.
const BGRA_UNMULTIPLIED: u32 = 4 | (2 << 12);

type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, length: isize) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDataProviderCreateWithCFData(data: CFTypeRef) -> CFTypeRef;
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        info: u32,
        provider: CFTypeRef,
        decode: *const f64,
        interpolate: bool,
        intent: i32,
    ) -> CFTypeRef;
    static kCGColorSpaceDisplayP3: CFTypeRef;
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    static NSDraggingImageComponentIconKey: Id;
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "NRFileDragSource"]
    struct Source;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for Source {}

    impl Source {
        /// Copy only (`NSDraggingSource`). Generic would let Finder move files out of the drive
        /// on the same volume.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operations(&self, _session: *mut AnyObject, _context: isize) -> usize {
            COPY
        }
    }
);

thread_local! {
    /// The shared source for every session, since a session doesn't retain its source.
    static SOURCE: Retained<Source> = {
        let main_thread = MainThreadMarker::new().expect("a drag begins on the main thread");
        // SAFETY: NSObject's designated initialiser.
        unsafe { msg_send![super(Source::alloc(main_thread).set_ivars(())), init] }
    };
}

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

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Point {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Size {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Rect {
    origin: Point,
    size: Size,
}

// SAFETY: Core Graphics' `CGPoint`, `CGSize` and `CGRect`, of doubles.
unsafe impl objc2::encode::Encode for Point {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl objc2::encode::Encode for Size {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}
unsafe impl objc2::encode::Encode for Rect {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
}

/// Starts a drag of `files` from `window` at the current mouse event. False when there is no
/// left-button event to start from (GPUI replays drags on a timer); the next real move starts it.
pub fn drag_files_out(files: &[Carried], window: &mut gpui::Window, _: &mut gpui::App) -> bool {
    let view: Id = match HasWindowHandle::window_handle(&*window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    };
    // SAFETY: used on the main thread while the event is handled; everything created here is
    // released or autoreleased.
    unsafe {
        let event: Id = msg_send![application(), currentEvent];
        if event.is_null() {
            return false;
        }
        let kind: usize = msg_send![event, r#type];
        if kind != LEFT_MOUSE_DRAGGED && kind != LEFT_MOUSE_DOWN {
            return false;
        }
        let in_window: Point = msg_send![event, locationInWindow];
        let at: Point =
            msg_send![view, convertPoint: in_window, fromView: std::ptr::null_mut::<AnyObject>()];
        let items = dragging_items(files, at);
        let session: Id = SOURCE.with(|source| {
            msg_send![
                view,
                beginDraggingSessionWithItems: items,
                event: event,
                source: &**source
            ]
        });
        if session.is_null() {
            return false;
        }
        // The drag left the window, so dropping it nowhere just ends it.
        let _: () = msg_send![session, setAnimatesToStartingPositionsOnCancelOrFail: false];
        true
    }
}

/// An autoreleased array of dragging items for `files`, each the file's `NSURL` in a
/// `PICTURE_POINTS` square centred on `at` (source view points). Images are drawn lazily, so a
/// large selection starts dragging at once.
fn dragging_items(files: &[Carried], at: Point) -> Id {
    let half = PICTURE_POINTS / 2.0;
    let frame = Rect {
        origin: Point {
            x: at.x - half,
            y: at.y - half,
        },
        size: Size {
            width: PICTURE_POINTS,
            height: PICTURE_POINTS,
        },
    };
    // SAFETY: each item is released once the array retains it; the array is autoreleased.
    unsafe {
        let items: Id = msg_send![class!(NSMutableArray), array];
        for file in files {
            let path = file.path.to_string_lossy().into_owned();
            let url: Id = msg_send![class!(NSURL), fileURLWithPath: ns_string(&path)];
            let item: Id = msg_send![class!(NSDraggingItem), alloc];
            let item: Id = msg_send![item, initWithPasteboardWriter: url];
            let _: () = msg_send![item, setDraggingFrame: frame];
            let picture = file.picture.clone();
            let draw = RcBlock::new(move || -> Id { components(&path, picture.as_ref()) });
            let _: () = msg_send![item, setImageComponentsProvider: &*draw];
            let _: () = msg_send![items, addObject: item];
            let _: () = msg_send![item, release];
        }
        items
    }
}

/// An autoreleased item image: the thumbnail fitted to the square, else the file icon.
fn components(path: &str, picture: Option<&Arc<gpui::RenderImage>>) -> Id {
    // SAFETY: created on the main thread and autoreleased.
    unsafe {
        let (image, size) = match picture.and_then(|picture| image_of(picture)) {
            Some(drawn) => drawn,
            None => {
                let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
                let icon: Id = msg_send![workspace, iconForFile: ns_string(path)];
                let square = Size {
                    width: PICTURE_POINTS,
                    height: PICTURE_POINTS,
                };
                (icon, square)
            }
        };
        let component: Id = msg_send![
            class!(NSDraggingImageComponent),
            draggingImageComponentWithKey: NSDraggingImageComponentIconKey
        ];
        let _: () = msg_send![component, setContents: image];
        let frame = Rect {
            origin: Point {
                x: (PICTURE_POINTS - size.width) / 2.0,
                y: (PICTURE_POINTS - size.height) / 2.0,
            },
            size,
        };
        let _: () = msg_send![component, setFrame: frame];
        msg_send![class!(NSArray), arrayWithObject: component]
    }
}

/// `picture` (Display P3) as an autoreleased `NSImage`, with its size fitted to the square.
fn image_of(picture: &gpui::RenderImage) -> Option<(Id, Size)> {
    let bytes = picture.as_bytes(0)?;
    let pixels = picture.size(0);
    let (width, height) = (pixels.width.0 as usize, pixels.height.0 as usize);
    if width == 0 || height == 0 || bytes.len() < width * height * 4 {
        return None;
    }
    let fit = PICTURE_POINTS / width.max(height) as f64;
    let size = Size {
        width: width as f64 * fit,
        height: height as f64 * fit,
    };
    // SAFETY: CG objects created here are released here; the image copies the bytes.
    unsafe {
        let data = CFDataCreate(
            std::ptr::null(),
            bytes.as_ptr(),
            (width * height * 4) as isize,
        );
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        let space = CGColorSpaceCreateWithName(kCGColorSpaceDisplayP3);
        let cg_image = CGImageCreate(
            width,
            height,
            8,
            32,
            width * 4,
            space,
            BGRA_UNMULTIPLIED,
            provider,
            std::ptr::null(),
            true,
            0,
        );
        CFRelease(space);
        CFRelease(provider);
        if cg_image.is_null() {
            return None;
        }
        let image: Id = msg_send![class!(NSImage), alloc];
        let image: Id = msg_send![image, initWithCGImage: cg_image.cast::<CGImage>(), size: size];
        CFRelease(cg_image);
        let image: Id = msg_send![image, autorelease];
        Some((image, size))
    }
}

#[cfg(test)]
mod tests {
    use super::{Carried, Id, PICTURE_POINTS, Point, Rect, dragging_items};
    use objc2::runtime::Bool;
    use objc2::{class, msg_send};
    use std::sync::Arc;

    /// Two files make two items, each with its URL and either the fitted thumbnail or the icon.
    #[test]
    fn each_file_is_dragged_as_its_url_with_its_picture_or_icon() {
        let folder = std::env::temp_dir().join("northroom drag out");
        std::fs::create_dir_all(&folder).expect("the temporary folder takes a folder");
        let (still, notes) = (folder.join("still.png"), folder.join("notes.txt"));
        for file in [&still, &notes] {
            std::fs::write(file, "x").expect("the folder takes a file");
        }
        // A 2:1 image in GPUI's format.
        let picture = image::RgbaImage::from_pixel(8, 4, image::Rgba([40, 80, 160, 255]));
        let picture = Arc::new(gpui::RenderImage::new([image::Frame::new(picture)]));
        let files = [
            Carried {
                path: still.clone(),
                picture: Some(picture),
            },
            Carried {
                path: notes.clone(),
                picture: None,
            },
        ];
        objc2::rc::autoreleasepool(|_| {
            let items = dragging_items(&files, Point { x: 100.0, y: 50.0 });
            // SAFETY: the new items, read inside their autorelease pool.
            unsafe {
                let count: usize = msg_send![items, count];
                assert_eq!(count, 2);
                for (index, (file, drawn)) in [(&still, (64.0, 32.0)), (&notes, (64.0, 64.0))]
                    .into_iter()
                    .enumerate()
                {
                    let item: Id = msg_send![items, objectAtIndex: index];
                    let url: Id = msg_send![item, item];
                    let is_file: Bool = msg_send![url, isFileURL];
                    assert!(is_file.as_bool());
                    let path: Id = msg_send![url, path];
                    let path: *const std::ffi::c_char = msg_send![path, UTF8String];
                    let path = std::ffi::CStr::from_ptr(path).to_string_lossy();
                    assert_eq!(std::path::Path::new(&*path), file.as_path());
                    let frame: Rect = msg_send![item, draggingFrame];
                    let half = PICTURE_POINTS / 2.0;
                    assert_eq!(
                        (frame.origin.x, frame.origin.y, frame.size.width),
                        (100.0 - half, 50.0 - half, PICTURE_POINTS)
                    );
                    let components: Id = msg_send![item, imageComponents];
                    let component: Id = msg_send![components, firstObject];
                    let contents: Id = msg_send![component, contents];
                    let is_image: Bool = msg_send![contents, isKindOfClass: class!(NSImage)];
                    assert!(is_image.as_bool());
                    let shown: Rect = msg_send![component, frame];
                    assert_eq!((shown.size.width, shown.size.height), drawn);
                }
            }
        });
        std::fs::remove_dir_all(&folder).expect("the folder made is removed");
    }
}
