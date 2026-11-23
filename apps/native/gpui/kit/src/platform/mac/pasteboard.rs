//! Pasteboards for `platform/clipboard.rs`: a named one for checks, read and written freely,
//! and the general pasteboard, which takes files and is only probed for the types it holds.

use super::appkit::{Id, file_urls, ns_string, path_of_url};
use objc2::{class, msg_send};
use std::path::PathBuf;

/// The checks' named pasteboard.
const CHECK: &str = "com.northroom.native.check";
/// `NSPasteboardTypeString`.
const WORDS: &str = "public.utf8-plain-text";
/// `NSPasteboardTypeFileURL`.
const FILE: &str = "public.file-url";

fn check() -> Id {
    // SAFETY: created on first use and retained by the system.
    unsafe { msg_send![class!(NSPasteboard), pasteboardWithName: ns_string(CHECK)] }
}

/// Puts `text` on the checks' pasteboard.
pub fn check_write(text: &str) {
    // SAFETY: a live pasteboard and autoreleased strings.
    unsafe {
        let pasteboard = check();
        let _: isize = msg_send![pasteboard, clearContents];
        let _: bool = msg_send![pasteboard, setString: ns_string(text), forType: ns_string(WORDS)];
    }
}

/// The text on the checks' pasteboard, if any.
pub fn check_read() -> Option<String> {
    // SAFETY: a live pasteboard; the UTF-8 bytes live as long as the returned `NSString`.
    unsafe {
        let words: Id = msg_send![check(), stringForType: ns_string(WORDS)];
        if words.is_null() {
            return None;
        }
        // Null when the text can't be encoded as UTF-8.
        let bytes: *const std::ffi::c_char = msg_send![words, UTF8String];
        (!bytes.is_null()).then(|| {
            std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .into_owned()
        })
    }
}

fn general() -> Id {
    // SAFETY: the system's general pasteboard.
    unsafe { msg_send![class!(NSPasteboard), generalPasteboard] }
}

/// Whether `pasteboard` holds `kind`. Reads only the types, not the contents.
fn holds(pasteboard: Id, kind: &str) -> bool {
    // SAFETY: reads only a live pasteboard's types.
    unsafe {
        let types: Id = msg_send![class!(NSArray), arrayWithObject: ns_string(kind)];
        let found: Id = msg_send![pasteboard, availableTypeFromArray: types];
        !found.is_null()
    }
}

/// Whether the general pasteboard holds text. Reads only the types.
pub fn general_has_words() -> bool {
    holds(general(), WORDS)
}

/// Whether the general pasteboard holds files. Reads only the types.
pub fn general_has_files() -> bool {
    holds(general(), FILE)
}

/// Whether the checks' pasteboard holds files.
pub fn check_has_files() -> bool {
    holds(check(), FILE)
}

/// Puts `paths` on `pasteboard` as Finder copies them, one item per file, plus the paths as
/// text, one per line, for terminals and text fields.
fn write_files_to(pasteboard: Id, paths: &[PathBuf]) {
    let lines: Vec<String> = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    // SAFETY: a live pasteboard and autoreleased URLs and strings. The text goes on the first
    // item, which is the one text readers read.
    unsafe {
        let _: isize = msg_send![pasteboard, clearContents];
        let _: bool = msg_send![pasteboard, writeObjects: file_urls(paths)];
        let _: bool = msg_send![
            pasteboard,
            setString: ns_string(&lines.join("\n")),
            forType: ns_string(WORDS)
        ];
    }
}

/// Puts `paths` on the general pasteboard (`write_files_to`).
pub fn write_files(paths: &[PathBuf]) {
    write_files_to(general(), paths);
}

/// Puts `paths` on the checks' pasteboard (`write_files_to`).
pub fn check_write_files(paths: &[PathBuf]) {
    write_files_to(check(), paths);
}

/// The files on the checks' pasteboard, in order.
pub fn check_read_files() -> Vec<PathBuf> {
    // SAFETY: a live pasteboard; the URLs are autoreleased file URLs (`…FileURLsOnlyKey`).
    unsafe {
        let classes: Id = msg_send![class!(NSArray), arrayWithObject: class!(NSURL)];
        let yes: Id = msg_send![class!(NSNumber), numberWithBool: true];
        let options: Id = msg_send![
            class!(NSDictionary),
            dictionaryWithObject: yes,
            forKey: ns_string("NSPasteboardURLReadingFileURLsOnlyKey")
        ];
        let urls: Id = msg_send![check(), readObjectsForClasses: classes, options: options];
        if urls.is_null() {
            return Vec::new();
        }
        let count: usize = msg_send![urls, count];
        (0..count)
            .map(|index| path_of_url(msg_send![urls, objectAtIndex: index]))
            .collect()
    }
}

/// The general pasteboard's change count, for tests.
#[cfg(test)]
pub fn general_change_count() -> isize {
    // SAFETY: reads only the change count.
    unsafe { msg_send![general(), changeCount] }
}

/// The checks' pasteboard change count, for tests.
#[cfg(test)]
pub fn check_change_count() -> isize {
    // SAFETY: reads a live pasteboard's change count.
    unsafe { msg_send![check(), changeCount] }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Files also appear as the legacy filenames list, which GPUI reads for
    /// `ClipboardEntry::ExternalPaths`, and their paths are the text.
    #[test]
    fn files_written_are_read_as_gpui_reads_them() {
        let folder = std::env::temp_dir();
        let paths = vec![folder.join("a picture ✓.png"), folder.join("Notes")];
        // SAFETY: a test-owned pasteboard, released at the end; its results are autoreleased.
        unsafe {
            let pasteboard: Id = msg_send![
                class!(NSPasteboard),
                pasteboardWithName: ns_string("com.northroom.native.test.files")
            ];
            write_files_to(pasteboard, &paths);
            let names: Id = msg_send![
                pasteboard,
                propertyListForType: ns_string("NSFilenamesPboardType")
            ];
            assert!(!names.is_null(), "the files are there as file names");
            let count: usize = msg_send![names, count];
            let read: Vec<PathBuf> = (0..count)
                .map(|index| {
                    let name: Id = msg_send![names, objectAtIndex: index];
                    let bytes: *const std::ffi::c_char = msg_send![name, UTF8String];
                    PathBuf::from(std::ffi::CStr::from_ptr(bytes).to_string_lossy().as_ref())
                })
                .collect();
            let words: Id = msg_send![pasteboard, stringForType: ns_string(WORDS)];
            let bytes: *const std::ffi::c_char = msg_send![words, UTF8String];
            let words = std::ffi::CStr::from_ptr(bytes)
                .to_string_lossy()
                .into_owned();
            let _: () = msg_send![pasteboard, releaseGlobally];
            assert_eq!(read, paths);
            assert_eq!(
                words,
                format!("{}\n{}", paths[0].display(), paths[1].display())
            );
        }
    }
}
