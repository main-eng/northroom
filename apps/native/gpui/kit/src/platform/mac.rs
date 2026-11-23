//! macOS platform services: thumbnails, file metadata, the Trash, windows for checks, update
//! signatures and the OS version. Thin calls into the system frameworks; no app logic.

use northroom_core::Relock;
use std::ffi::{CStr, c_char, c_void};

mod accessibility;
mod alert;
mod appkit;
mod drag;
mod files;
mod menu_items;
pub(super) mod pasteboard;
mod sampler;
mod scripted;
mod speech;
mod thumbnail;
mod tray;

pub use accessibility::{
    forget_hosted, hosted_element, report_accessibility_focus, show_hosted, show_placeholder,
};
pub use alert::{Alert, AlertStyle, run_alert};
pub use drag::drag_files_out;
pub use files::{reveal, share};
pub use menu_items::{added_items, hide_items, paste_items};
pub use pasteboard::write_files;
pub use sampler::sample_colour;
pub use scripted::{Button, hosted_has_keys, post_key, post_press};
pub use speech::{start_speaking, stop_speaking};
pub use thumbnail::system_thumbnail;
pub use tray::{Tray, show_tray};

type CFTypeRef = *const c_void;
type CFIndex = isize;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFArrayGetCount(array: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: CFTypeRef, index: CFIndex) -> CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFTypeRef, text: *const c_char, encoding: u32)
    -> CFTypeRef;
    fn CFStringGetLength(text: CFTypeRef) -> CFIndex;
    fn CFStringGetCString(
        text: CFTypeRef,
        buffer: *mut c_char,
        size: CFIndex,
        encoding: u32,
    ) -> bool;
    fn CFURLCreateFromFileSystemRepresentation(
        alloc: CFTypeRef,
        path: *const u8,
        length: CFIndex,
        directory: bool,
    ) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> CFTypeRef;
    fn CFNumberCreate(alloc: CFTypeRef, kind: CFIndex, value: *const c_void) -> CFTypeRef;
    fn CFPreferencesSetAppValue(key: CFTypeRef, value: CFTypeRef, application: CFTypeRef);
    fn CFPreferencesAppSynchronize(application: CFTypeRef) -> bool;
    static kCFPreferencesCurrentApplication: CFTypeRef;
    static kCFBooleanTrue: CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGColorSpaceCreateWithName(name: CFTypeRef) -> CFTypeRef;
    fn CGColorSpaceCreateDeviceGray() -> CFTypeRef;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits: usize,
        row: usize,
        space: CFTypeRef,
        info: u32,
    ) -> CFTypeRef;
    fn CGContextDrawImage(context: CFTypeRef, rect: CGRect, image: CFTypeRef);
    fn CGImageRelease(image: CFTypeRef);
    static kCGColorSpaceDisplayP3: CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCreateWithURL(url: CFTypeRef, options: CFTypeRef) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    static kCGImageSourceCreateThumbnailFromImageAlways: CFTypeRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFTypeRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFTypeRef;
}

#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

// SAFETY: Core Graphics' `CGRect`, a `CGPoint` then a `CGSize`, each two doubles.
unsafe impl objc2::encode::Encode for CGRect {
    const ENCODING: objc2::encode::Encoding = objc2::encode::Encoding::Struct(
        "CGRect",
        &[
            objc2::encode::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]),
            objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]),
        ],
    );
}

const UTF8: u32 = 0x0800_0100;

fn cf_string(text: &str) -> CFTypeRef {
    let owned = crate::core::c(text);
    // SAFETY: a NUL-terminated UTF-8 string.
    unsafe { CFStringCreateWithCString(std::ptr::null(), owned.as_ptr(), UTF8) }
}

fn string_of(text: CFTypeRef) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: `text` is a CFString; the buffer holds every UTF-8 byte of it.
    unsafe {
        let capacity = CFStringGetLength(text) * 4 + 1;
        let mut buffer = vec![0 as c_char; capacity as usize];
        if !CFStringGetCString(text, buffer.as_mut_ptr(), capacity, UTF8) {
            return String::new();
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// The parent folders for the app's own: `~/Library/Application Support`, `Logs` and `Caches`
/// (`nr_paths_open`).
pub fn app_folders() -> (String, String, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let library = format!("{home}/Library");
    (
        format!("{library}/Application Support"),
        format!("{library}/Logs"),
        format!("{library}/Caches"),
    )
}

/// Seconds from the Unix epoch to Core Foundation's reference date (2001).
const REFERENCE_DATE: f64 = 978_307_200.0;

/// `kCFNumberDoubleType`.
const DOUBLE: CFIndex = 13;

/// Image pixels in the window's colour space (`crate::display`): BGRA bytes, straight alpha as
/// GPUI expects, top row first.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Draws `image` (a `CGImage`) from its own profile into the window's colour space, Display P3,
/// as AppKit does, so wide-gamut images keep their colours.
fn pixels_of(image: CFTypeRef) -> Option<Pixels> {
    // SAFETY: `image` is a CGImage; the bitmap context draws into a buffer of its size.
    unsafe {
        let (width, height) = (CGImageGetWidth(image), CGImageGetHeight(image));
        if width == 0 || height == 0 {
            return None;
        }
        let mut bgra = vec![0u8; width * height * 4];
        let space = CGColorSpaceCreateWithName(kCGColorSpaceDisplayP3);
        if space.is_null() {
            return None;
        }
        // Premultiplied alpha last, 32-bit big-endian: R, G, B, A in memory.
        let info = 1 | (4 << 12);
        let context = CGBitmapContextCreate(
            bgra.as_mut_ptr().cast(),
            width,
            height,
            8,
            width * 4,
            space,
            info,
        );
        CFRelease(space);
        if context.is_null() {
            return None;
        }
        CGContextDrawImage(
            context,
            CGRect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            },
            image,
        );
        CFRelease(context);
        // Core Graphics only draws premultiplied; GPUI wants straight alpha.
        for pixel in bgra.as_chunks_mut::<4>().0 {
            gpui::swap_rgba_pa_to_bgra(pixel);
        }
        Some(Pixels {
            width: width as u32,
            height: height as u32,
            bgra,
        })
    }
}

/// The image at `path`, at most `pixels` on its long side.
pub fn image_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    // SAFETY: Core Foundation objects made here are released here.
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            path.as_ptr(),
            path.len() as CFIndex,
            false,
        );
        if url.is_null() {
            return None;
        }
        let source = CGImageSourceCreateWithURL(url, std::ptr::null());
        CFRelease(url);
        if source.is_null() {
            return None;
        }
        let size = i64::from(pixels);
        let number = CFNumberCreate(std::ptr::null(), 4, (&size as *const i64).cast());
        let keys = [
            kCGImageSourceCreateThumbnailFromImageAlways,
            kCGImageSourceCreateThumbnailWithTransform,
            kCGImageSourceThumbnailMaxPixelSize,
        ];
        let values = [kCFBooleanTrue, kCFBooleanTrue, number];
        let options = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            3,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        let image = CGImageSourceCreateThumbnailAtIndex(source, 0, options);
        for made in [options, number, source] {
            if !made.is_null() {
                CFRelease(made);
            }
        }
        if image.is_null() {
            return None;
        }
        let pixels = pixels_of(image);
        CGImageRelease(image);
        pixels
    }
}

/// The frame `seconds` into the video at `path`, at most `pixels` on either side, from the
/// core's decoder (`nr_video_still`) so every playable format works.
pub fn video_still(path: &str, seconds: f64, pixels: u32) -> Option<Pixels> {
    let named = crate::core::c(path);
    // SAFETY: a NUL-terminated path; the core hands back a CGImage the caller releases.
    unsafe {
        let image = northroom_core::ffi::nr_video_still(named.as_ptr(), seconds, pixels);
        if image.is_null() {
            return None;
        }
        let made = pixels_of(image);
        CGImageRelease(image);
        made
    }
}

/// Keeps a check out of the Dock and the app switcher (`setActivationPolicy(.accessory)`).
pub fn keep_out_of_the_dock() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: called on the main thread once the application object exists.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let _: bool = msg_send![application, setActivationPolicy: 1isize];
    }
}

/// Shows `count` as the Dock icon badge; no badge for 0.
pub fn badge_dock(count: usize) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let label = if count == 0 {
        std::ptr::null()
    } else {
        cf_string(&count.to_string())
    };
    // SAFETY: called on the main thread; the label is a CFString, toll-free bridged to the
    // NSString the tile copies, or nil for no badge.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let tile: *mut AnyObject = msg_send![application, dockTile];
        let _: () = msg_send![tile, setBadgeLabel: label as *mut AnyObject];
        if !label.is_null() {
            CFRelease(label);
        }
    }
}

/// Moves a check's window off every screen, `width` by `height` points and behind all other
/// windows, so it never covers the user's windows or takes their clicks.
///
/// The window opens hidden (`show: false`) and is ordered in here already offscreen. Offscreen,
/// GPUI renders at 2x whatever the displays are (a CI runner's is 1x). Checks read GPUI's own
/// frames (`render_to_image`) and post input to the app's own queue, so the window never needs
/// to be visible. AppKit keeps titled windows on a screen, so the title bar is removed first.
/// The page host's stage windows do the same (`StageWindow::offscreen`).
pub fn put_check_window_off_screen(window: &gpui::Window, width: f64, height: f64) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    /// `NSWindowStyleMaskTitled`.
    const TITLED: usize = 1;
    let view: *mut AnyObject = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    };
    // SAFETY: window, screens and frames are read and set on the main thread.
    unsafe {
        let native: *mut AnyObject = msg_send![view, window];
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let right = (0..count).fold(0.0f64, |right, index| {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: index];
            let frame: CGRect = msg_send![screen, frame];
            right.max(frame.x + frame.width)
        });
        let mask: usize = msg_send![native, styleMask];
        let _: () = msg_send![native, setStyleMask: mask & !TITLED];
        let away = CGRect {
            x: right + width,
            y: 0.0,
            width,
            height,
        };
        let _: () = msg_send![native, setFrame: away, display: false];
        let _: () = msg_send![native, orderBack: std::ptr::null_mut::<AnyObject>()];
    }
}

/// Floats the window above other apps on every Space so it keeps getting frames while another
/// app is in front; covered windows get none. Only for manual frame timing (`NR_FRAME_TIMES`),
/// never for checks.
pub fn float_over_others(window: &gpui::Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    /// `NSFloatingWindowLevel`.
    const FLOATING: isize = 3;
    /// `NSWindowCollectionBehaviorCanJoinAllSpaces | NSWindowCollectionBehaviorFullScreenAuxiliary`.
    const EVERY_SPACE: usize = 1 | 1 << 8;
    let view: *mut AnyObject = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::AppKit(handle)) => handle.ns_view.as_ptr().cast(),
        _ => panic!("a GPUI window on a Mac has an AppKit view"),
    };
    // SAFETY: set on the main thread.
    unsafe {
        let native: *mut AnyObject = msg_send![view, window];
        let _: () = msg_send![native, setLevel: FLOATING];
        let _: () = msg_send![native, setCollectionBehavior: EVERY_SPACE];
        let _: () = msg_send![native, orderFrontRegardless];
    }
}

/// Whether Reduce Motion is on (`accessibilityDisplayShouldReduceMotion`); spinners stop.
pub fn reduce_motion() -> bool {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: read on the main thread.
    unsafe {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        msg_send![workspace, accessibilityDisplayShouldReduceMotion]
    }
}

// MARK: The menu bar's system commands

/// Sends a standard AppKit action to the application, as a menu item would
/// (`orderFrontStandardAboutPanel:`, `arrangeInFront:`).
fn tell_application(command: objc2::runtime::Sel) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: the application responds to both actions; called on the main thread.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let nothing: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![application, performSelector: command, withObject: nothing];
    }
}

/// Shows the standard About panel.
pub fn show_about_panel() {
    tell_application(objc2::sel!(orderFrontStandardAboutPanel:));
}

/// Bring All to Front.
pub fn bring_all_to_front() {
    tell_application(objc2::sel!(arrangeInFront:));
}

/// Tells AppKit which menus, by index, are the Window menu and the Help menu.
pub fn name_system_menus(windows: Option<usize>, help: Option<usize>) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: read on the main thread; indexes are always in range because the caller numbers
    // the menus it set.
    unsafe {
        let application: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let main: *mut AnyObject = msg_send![application, mainMenu];
        if main.is_null() {
            return;
        }
        let submenu = |index: usize| -> *mut AnyObject {
            let item: *mut AnyObject = msg_send![main, itemAtIndex: index as isize];
            msg_send![item, submenu]
        };
        if let Some(index) = windows {
            let _: () = msg_send![application, setWindowsMenu: submenu(index)];
        }
        if let Some(index) = help {
            let _: () = msg_send![application, setHelpMenu: submenu(index)];
        }
    }
}

// MARK: File metadata from Finder and Spotlight

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFURLCopyResourcePropertyForKey(
        url: CFTypeRef,
        key: CFTypeRef,
        value: *mut CFTypeRef,
        error: *mut CFTypeRef,
    ) -> bool;
    fn CFGetTypeID(value: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFDateGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFNumberGetValue(number: CFTypeRef, kind: CFIndex, value: *mut c_void) -> bool;
    fn CFDateGetAbsoluteTime(date: CFTypeRef) -> f64;
    static kCFURLLocalizedTypeDescriptionKey: CFTypeRef;
}

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn MDItemCreate(alloc: CFTypeRef, path: CFTypeRef) -> CFTypeRef;
    fn MDItemCopyAttribute(item: CFTypeRef, name: CFTypeRef) -> CFTypeRef;
    static kMDItemDateAdded: CFTypeRef;
    static kMDItemLastUsedDate: CFTypeRef;
    static kMDItemPixelWidth: CFTypeRef;
    static kMDItemPixelHeight: CFTypeRef;
    static kMDItemDurationSeconds: CFTypeRef;
    static kMDItemAcquisitionModel: CFTypeRef;
    static kMDItemAuthors: CFTypeRef;
    static kMDItemFinderComment: CFTypeRef;
    static kMDItemWhereFroms: CFTypeRef;
}

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    fn CGImageSourceCopyPropertiesAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFTypeRef,
    ) -> CFTypeRef;
    static kCGImagePropertyPixelWidth: CFTypeRef;
    static kCGImagePropertyPixelHeight: CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetValue(dictionary: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
}

fn path_of(path: *const c_char) -> Option<String> {
    // SAFETY: the core passes a NUL-terminated path, or null.
    (!path.is_null()).then(|| {
        unsafe { CStr::from_ptr(path) }
            .to_string_lossy()
            .into_owned()
    })
}

fn file_url(path: &str) -> CFTypeRef {
    // SAFETY: path bytes; the caller releases the URL.
    unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            path.as_ptr(),
            path.len() as CFIndex,
            false,
        )
    }
}

/// Writes as much of `text` as fits into the core's buffer.
fn hand_over(text: &str, out: *mut u8, capacity: usize) -> usize {
    let bytes = &text.as_bytes()[..text.len().min(capacity)];
    // SAFETY: the core passes room for `capacity` bytes at `out`.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    bytes.len()
}

/// A resource value of `path`; the caller releases it.
fn resource(path: &str, key: CFTypeRef) -> CFTypeRef {
    let url = file_url(path);
    let mut value: CFTypeRef = std::ptr::null();
    if url.is_null() {
        return value;
    }
    // SAFETY: a file URL and a resource key; the URL is released here.
    unsafe {
        CFURLCopyResourcePropertyForKey(url, key, &mut value, std::ptr::null_mut());
        CFRelease(url);
    }
    value
}

fn strings_of(array: CFTypeRef) -> Vec<String> {
    // SAFETY: `array` is a CFArray.
    unsafe {
        (0..CFArrayGetCount(array))
            .map(|index| string_of(CFArrayGetValueAtIndex(array, index)))
            .collect()
    }
}

/// File system callbacks passed to the core (`nr_drive_start`): file index, kind names, Trash.
pub fn file_system() -> northroom_core::ffi::NrFileSystem {
    northroom_core::ffi::NrFileSystem {
        file_index: Some(file_index),
        describe_kind: Some(describe_kind),
        trash: Some(trash),
    }
}

/// `trash` result when nothing was moved.
const NOT_TRASHED: i64 = -1;

/// Moves a file or folder to the Trash so Finder's Put Back works
/// (`-[NSFileManager trashItemAtURL:resultingItemURL:error:]`). Writes the new path into
/// `landed` and returns its length in bytes, or `NOT_TRASHED`.
extern "C" fn trash(path: *const c_char, landed: *mut u8, capacity: usize) -> i64 {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let Some(path) = path_of(path) else {
        return NOT_TRASHED;
    };
    let url = file_url(&path);
    if url.is_null() {
        return NOT_TRASHED;
    }
    // SAFETY: CFURL is toll-free bridged to NSURL; our URL is released here and the returned
    // one is autoreleased.
    let resulting = unsafe {
        let manager: *mut AnyObject = msg_send![class!(NSFileManager), defaultManager];
        let mut resulting: *mut AnyObject = std::ptr::null_mut();
        let trashed: bool = msg_send![
            manager,
            trashItemAtURL: url.cast::<AnyObject>(),
            resultingItemURL: &raw mut resulting,
            error: std::ptr::null_mut::<*mut AnyObject>()
        ];
        CFRelease(url);
        if !trashed {
            return NOT_TRASHED;
        }
        resulting
    };
    if resulting.is_null() {
        return 0;
    }
    // SAFETY: the file URL returned by the Trash call.
    let now = unsafe { appkit::path_of_url(resulting) };
    hand_over(&now.to_string_lossy(), landed, capacity) as i64
}

/// Finder's kind name for a file ("PNG image"). Plain files are cached by extension so a
/// 5,000-file folder isn't 5,000 Launch Services lookups; links, apps and extensionless files
/// are looked up every time.
extern "C" fn describe_kind(path: *const c_char, out: *mut u8, capacity: usize) -> usize {
    static BY_EXTENSION: std::sync::Mutex<Option<std::collections::HashMap<String, String>>> =
        std::sync::Mutex::new(None);
    let Some(path) = path_of(path) else { return 0 };
    let file = std::path::Path::new(&path);
    let extension = std::fs::symlink_metadata(file)
        .is_ok_and(|stat| stat.is_file())
        .then(|| {
            file.extension()
                .map(|extension| extension.to_string_lossy().into_owned())
        })
        .flatten()
        .filter(|extension| !extension.is_empty());
    if let Some(extension) = &extension
        && let Some(text) = BY_EXTENSION.relock().get_or_insert_default().get(extension)
    {
        return hand_over(text, out, capacity);
    }
    let words = resource(&path, unsafe { kCFURLLocalizedTypeDescriptionKey });
    if words.is_null() {
        return 0;
    }
    let text = string_of(words);
    // SAFETY: a value this call copied.
    unsafe { CFRelease(words) };
    let written = hand_over(&text, out, capacity);
    if let Some(extension) = extension {
        BY_EXTENSION
            .relock()
            .get_or_insert_default()
            .insert(extension, text);
    }
    written
}

fn number_of(value: CFTypeRef) -> Option<f64> {
    let mut number = 0.0f64;
    // SAFETY: `value` is checked to be a CFNumber first.
    unsafe {
        (CFGetTypeID(value) == CFNumberGetTypeID()
            && CFNumberGetValue(value, DOUBLE, (&raw mut number).cast()))
        .then_some(number)
    }
}

/// Spotlight metadata for a file, one `name<TAB>value` per line (`file_system`).
extern "C" fn file_index(path: *const c_char, out: *mut u8, capacity: usize) -> usize {
    let Some(path) = path_of(path) else { return 0 };
    // One value per line, so a value containing a line break would split in two.
    let text = index_lines(&path)
        .iter()
        .map(|line| line.replace('\n', " "))
        .collect::<Vec<_>>()
        .join("\n");
    hand_over(&text, out, capacity)
}

/// Spotlight metadata for `path`: date added, last opened, size, duration, camera, authors,
/// comment and sources.
fn index_lines(path: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let item = IndexedFile::open(path);
    // SAFETY: the attribute names are constants the system defines.
    unsafe {
        item.push_date(kMDItemDateAdded, "added", &mut lines);
        item.push_date(kMDItemLastUsedDate, "opened", &mut lines);
        let size = item
            .number(kMDItemPixelWidth)
            .zip(item.number(kMDItemPixelHeight));
        let duration = item.number(kMDItemDurationSeconds);
        // Outside indexed folders, read image size from the file header.
        if let Some((width, height)) = size.or_else(|| picture_size(path)) {
            lines.push(format!("width\t{}", width as i64));
            lines.push(format!("height\t{}", height as i64));
        }
        if let Some(duration) = duration {
            lines.push(format!("duration\t{duration}"));
        }
        item.push_words(kMDItemAcquisitionModel, "camera", &mut lines);
        item.push_words(kMDItemAuthors, "author", &mut lines);
        item.push_words(kMDItemFinderComment, "comment", &mut lines);
        item.push_words(kMDItemWhereFroms, "source", &mut lines);
    }
    lines
}

/// A Spotlight item, released on drop. Has no attributes when the index has no item for it.
struct IndexedFile(CFTypeRef);

impl IndexedFile {
    fn open(path: &str) -> Self {
        // SAFETY: the path string is created and released here.
        unsafe {
            let name = cf_string(path);
            let item = MDItemCreate(std::ptr::null(), name);
            CFRelease(name);
            Self(item)
        }
    }

    /// A copy of attribute `key` for the caller to release; null when absent.
    fn attribute(&self, key: CFTypeRef) -> CFTypeRef {
        if self.0.is_null() {
            return std::ptr::null();
        }
        // SAFETY: a live item and an attribute name.
        unsafe { MDItemCopyAttribute(self.0, key) }
    }

    /// Adds `label<TAB>unix seconds` when attribute `key` is a date.
    fn push_date(&self, key: CFTypeRef, label: &str, lines: &mut Vec<String>) {
        let made = self.attribute(key);
        if made.is_null() {
            return;
        }
        // SAFETY: a value this call copied, read as a date after checking its type.
        unsafe {
            if CFGetTypeID(made) == CFDateGetTypeID() {
                lines.push(format!(
                    "{label}\t{}",
                    CFDateGetAbsoluteTime(made) + REFERENCE_DATE
                ));
            }
            CFRelease(made);
        }
    }

    /// Attribute `key` when it is a number.
    fn number(&self, key: CFTypeRef) -> Option<f64> {
        let made = self.attribute(key);
        if made.is_null() {
            return None;
        }
        let read = number_of(made);
        // SAFETY: a value this call copied.
        unsafe { CFRelease(made) };
        read
    }

    /// Adds `label<TAB>text` when attribute `key` is a string, or one line per string when it is
    /// a list.
    fn push_words(&self, key: CFTypeRef, label: &str, lines: &mut Vec<String>) {
        let made = self.attribute(key);
        if made.is_null() {
            return;
        }
        // SAFETY: a value this call copied, read according to its type.
        unsafe {
            if CFGetTypeID(made) == CFStringGetTypeID() {
                lines.push(format!("{label}\t{}", string_of(made)));
            } else if CFGetTypeID(made) == CFArrayGetTypeID() {
                lines.extend(
                    strings_of(made)
                        .into_iter()
                        .map(|each| format!("{label}\t{each}")),
                );
            }
            CFRelease(made);
        }
    }
}

impl Drop for IndexedFile {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper owns the item.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// The pixel size from the image header at `path`, for file kinds the core reads sizes of
/// (`nr_reads_picture_size`).
fn picture_size(path: &str) -> Option<(f64, f64)> {
    let kind = std::path::Path::new(path)
        .extension()
        .map(|kind| kind.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let kind = crate::core::c(&kind);
    // SAFETY: a NUL-terminated extension.
    if !unsafe { crate::nr::nr_reads_picture_size(kind.as_ptr()) } {
        return None;
    }
    let url = file_url(path);
    if url.is_null() {
        return None;
    }
    // SAFETY: a file URL and the image source made from it, both released here.
    unsafe {
        let source = CGImageSourceCreateWithURL(url, std::ptr::null());
        CFRelease(url);
        if source.is_null() {
            return None;
        }
        let size = header_size(source);
        CFRelease(source);
        size
    }
}

/// Pixel width and height of an image source's first image.
fn header_size(source: CFTypeRef) -> Option<(f64, f64)> {
    // SAFETY: a live image source; the properties copied from it are released here.
    unsafe {
        let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, std::ptr::null());
        if properties.is_null() {
            return None;
        }
        let read = |key| {
            let value = CFDictionaryGetValue(properties, key);
            if value.is_null() {
                None
            } else {
                number_of(value)
            }
        };
        let size = read(kCGImagePropertyPixelWidth).zip(read(kCGImagePropertyPixelHeight));
        CFRelease(properties);
        size
    }
}

/// Turns font smoothing off, matching `-webkit-font-smoothing: antialiased`.
///
/// Two readers of `AppleFontSmoothing` need it at 0. GPUI reads it once from the volatile
/// argument domain, which lasts only for the process, and otherwise thickens light glyphs.
/// Core Graphics otherwise smooths every glyph GPUI rasterizes (about 17% more ink), since GPUI
/// leaves the context's smoothing at its default. Core Graphics reads the stored value once per
/// process, at the first bitmap context, so we store it, make a context, and remove it again.
/// No setting outlives this call.
pub fn draw_glyphs_unsmoothed() {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: CF objects are toll-free bridged to the Foundation types the messages take; ours
    // are released here.
    unsafe {
        let key = cf_string("AppleFontSmoothing");
        let zero = 0i64;
        let value = CFNumberCreate(std::ptr::null(), 4, (&raw const zero).cast());
        let dictionary = CFDictionaryCreate(
            std::ptr::null(),
            &key,
            &value,
            1,
            &raw const kCFTypeDictionaryKeyCallBacks,
            &raw const kCFTypeDictionaryValueCallBacks,
        );
        let domain = cf_string("NSArgumentDomain");
        let defaults: *mut AnyObject = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let _: () = msg_send![defaults, setVolatileDomain: dictionary as *mut AnyObject, forName: domain as *mut AnyObject];
        let application = kCFPreferencesCurrentApplication;
        CFPreferencesSetAppValue(key, value, application);
        let gray = CGColorSpaceCreateDeviceGray();
        let mut pixel = 0u8;
        // An alpha-only context of one pixel (`kCGImageAlphaOnly`).
        let context = CGBitmapContextCreate((&raw mut pixel).cast(), 1, 1, 8, 1, gray, 7);
        CFPreferencesSetAppValue(key, std::ptr::null(), application);
        CFPreferencesAppSynchronize(application);
        for made in [context, gray, domain, dictionary, value, key] {
            CFRelease(made);
        }
    }
}

// MARK: Updates

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecStaticCodeCreateWithPath(path: CFTypeRef, flags: u32, code: *mut CFTypeRef) -> i32;
    fn SecRequirementCreateWithString(
        text: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> i32;
    fn SecStaticCodeCheckValidityWithErrors(
        code: CFTypeRef,
        flags: u32,
        requirement: CFTypeRef,
        errors: *mut CFTypeRef,
    ) -> i32;
    fn SecCopyErrorMessageString(status: i32, reserved: *const c_void) -> CFTypeRef;
    fn SecCodeCopySelf(flags: u32, code: *mut CFTypeRef) -> i32;
    fn SecCodeCopyStaticCode(code: CFTypeRef, flags: u32, stored: *mut CFTypeRef) -> i32;
    fn SecCodeCopyDesignatedRequirement(
        code: CFTypeRef,
        flags: u32,
        requirement: *mut CFTypeRef,
    ) -> i32;
}

/// `kSecCSCheckAllArchitectures | kSecCSCheckNestedCode | kSecCSStrictValidate`.
const WHOLE_SIGNATURE: u32 = 1 | 1 << 3 | 1 << 4;

/// The running app's version (`CFBundleShortVersionString`, empty outside a bundle), bundle
/// path and identifier.
pub fn running_bundle() -> (String, String, String) {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    // SAFETY: the main bundle's strings are toll-free bridged to CFStrings.
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        let key = cf_string("CFBundleShortVersionString");
        let version: *mut AnyObject =
            msg_send![bundle, objectForInfoDictionaryKey: key as *mut AnyObject];
        CFRelease(key);
        let path: *mut AnyObject = msg_send![bundle, bundlePath];
        let identifier: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        (
            string_of(version as CFTypeRef),
            string_of(path as CFTypeRef),
            string_of(identifier as CFTypeRef),
        )
    }
}

/// Writes the bundle identifier of the app at `path` into `out` (`NrBundleIdentifier`).
pub extern "C" fn bundle_identifier(path: *const c_char, out: *mut u8, capacity: usize) -> usize {
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send};
    let Some(path) = path_of(path) else {
        return 0;
    };
    // SAFETY: a bundle created from a path string and read on this thread.
    let identifier = unsafe {
        let named = cf_string(&path);
        let bundle: *mut AnyObject =
            msg_send![class!(NSBundle), bundleWithPath: named as *mut AnyObject];
        CFRelease(named);
        if bundle.is_null() {
            return 0;
        }
        let identifier: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        string_of(identifier as CFTypeRef)
    };
    hand_over(&identifier, out, capacity)
}

/// Validates the app at `app` strictly, nested code included, against `requirement` (in the
/// requirement language) or, when empty, the running app's designated requirement, as
/// Squirrel.Mac's `SQRLCodeSignature` does. Returns the result, the OS status and its message.
pub fn signature(
    app: &str,
    requirement: &str,
) -> (northroom_core::update::ffi::NrSignature, i32, String) {
    use northroom_core::update::ffi::NrSignature;
    // SAFETY: every object created here is released here; the calls write the out-pointers.
    unsafe {
        let url = file_url(app);
        // No URL means there is no code to check.
        if url.is_null() {
            return (NrSignature::Unreadable, 0, String::new());
        }
        let mut code: CFTypeRef = std::ptr::null();
        let opened = SecStaticCodeCreateWithPath(url, 0, &mut code);
        CFRelease(url);
        if opened != 0 || code.is_null() {
            return (NrSignature::Unreadable, opened, String::new());
        }
        let Some(wanted) = requirement_of(requirement) else {
            CFRelease(code);
            return (NrSignature::OwnUnreadable, 0, String::new());
        };
        let status = SecStaticCodeCheckValidityWithErrors(
            code,
            WHOLE_SIGNATURE,
            wanted,
            std::ptr::null_mut(),
        );
        CFRelease(wanted);
        CFRelease(code);
        if status == 0 {
            return (NrSignature::Passed, 0, String::new());
        }
        // The system's message: "a sealed resource is missing or invalid".
        let words = SecCopyErrorMessageString(status, std::ptr::null());
        let reason = string_of(words);
        if !words.is_null() {
            CFRelease(words);
        }
        (NrSignature::Failed, status, reason)
    }
}

/// The requirement to check against, for the caller to release: `requirement` compiled, or the
/// running app's designated requirement when empty. None when it can't be read.
fn requirement_of(requirement: &str) -> Option<CFTypeRef> {
    let mut wanted: CFTypeRef = std::ptr::null();
    let read = if requirement.is_empty() {
        own_requirement(&mut wanted)
    } else {
        // SAFETY: the string is created and released here; the call writes `wanted`.
        unsafe {
            let text = cf_string(requirement);
            let read = SecRequirementCreateWithString(text, 0, &mut wanted);
            CFRelease(text);
            read
        }
    };
    (read == 0 && !wanted.is_null()).then_some(wanted)
}

/// Writes the running app's designated requirement into `wanted` and returns the OS status.
fn own_requirement(wanted: &mut CFTypeRef) -> i32 {
    // SAFETY: both code objects are released here; the calls write the out-pointers.
    unsafe {
        let (mut running, mut stored): (CFTypeRef, CFTypeRef) =
            (std::ptr::null(), std::ptr::null());
        let mut read = SecCodeCopySelf(0, &mut running);
        if read == 0 {
            read = SecCodeCopyStaticCode(running, 0, &mut stored);
            CFRelease(running);
        }
        if read == 0 {
            read = SecCodeCopyDesignatedRequirement(stored, 0, wanted);
            CFRelease(stored);
        }
        read
    }
}

/// The OS major, minor and patch version for the diagnostic log, from
/// `kern.osproductversion` (what `operatingSystemVersion` reads). Missing parts are 0.
pub fn system_version() -> (u32, u32, u32) {
    let mut buffer = [0u8; 32];
    let mut length = buffer.len();
    // SAFETY: a NUL-terminated name, and a buffer `length` long that the call fills.
    let read = unsafe {
        libc::sysctlbyname(
            c"kern.osproductversion".as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    assert_eq!(read, 0, "the system says its version");
    let words = String::from_utf8_lossy(&buffer[..length.saturating_sub(1)]).into_owned();
    super::system::version_parts(&words)
}
