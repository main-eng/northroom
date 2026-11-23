//! Helpers shared by the AppKit modules: strings, sizes and the application.

use objc2::runtime::AnyObject;
use objc2::{class, msg_send};

pub type Id = *mut AnyObject;

/// `words` as an autoreleased `NSString`.
pub fn ns_string(words: &str) -> Id {
    let words = crate::core::c(words);
    // SAFETY: a NUL-terminated UTF-8 string, copied by the call.
    unsafe { msg_send![class!(NSString), stringWithUTF8String: words.as_ptr()] }
}

pub fn application() -> Id {
    // SAFETY: the application object exists once GPUI runs.
    unsafe { msg_send![class!(NSApplication), sharedApplication] }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

// SAFETY: Core Graphics' `CGSize`, two doubles.
unsafe impl objc2::encode::Encode for Size {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

/// `paths` as an autoreleased `NSArray` of file `NSURL`s.
pub fn file_urls(paths: &[std::path::PathBuf]) -> Id {
    // SAFETY: Foundation's array and URLs, built from autoreleased strings.
    unsafe {
        let urls: Id = msg_send![class!(NSMutableArray), arrayWithCapacity: paths.len()];
        for path in paths {
            let url: Id = msg_send![
                class!(NSURL),
                fileURLWithPath: ns_string(&path.to_string_lossy())
            ];
            let _: () = msg_send![urls, addObject: url];
        }
        urls
    }
}

/// The path of file URL `url`.
///
/// # Safety
/// `url` is a live file `NSURL`.
pub unsafe fn path_of_url(url: Id) -> std::path::PathBuf {
    // SAFETY: per the caller; the UTF-8 bytes live as long as the string.
    unsafe {
        let path: Id = msg_send![url, path];
        let bytes: *const std::ffi::c_char = msg_send![path, fileSystemRepresentation];
        let bytes = std::ffi::CStr::from_ptr(bytes).to_bytes();
        std::path::PathBuf::from(
            <std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes),
        )
    }
}
