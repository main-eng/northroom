//! Platform code shared by Windows and Linux, in plain Rust: Finder's kind names, and on Linux
//! image thumbnails via GPUI's decoders (the `image` crate; Windows uses its own, `windows.rs`).
//! macOS uses its frameworks instead (`mac.rs`).
//!
//! Also the pure parts of each platform's file services, kept here so they are tested on every
//! platform: grouping files by folder for the file manager, Windows' `DROPFILES` clipboard
//! block, and freedesktop.org `.trashinfo` records.

use std::ffi::{CStr, c_char};
use std::path::{Path, PathBuf};

#[cfg(not(target_os = "windows"))]
mod colour;

/// Finder's English kind name for an extension (any case), from Launch Services' type
/// descriptions, so every platform shows the same names. Unknown kinds are "Document", as in
/// Finder.
pub fn kind_of_extension(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "txt" | "c" | "cc" | "cpp" | "h" | "hpp" | "m" | "mm" | "java" | "php" | "pl" | "py"
        | "r" | "rb" | "sh" | "bash" | "zsh" | "swift" | "json" | "yaml" | "yml" => {
            "Plain Text Document"
        }
        "md" | "markdown" => "Markdown Document",
        "rtf" => "Rich Text Document",
        "pdf" => "PDF document",
        "html" | "htm" => "HTML text",
        "xml" => "XML document",
        "css" => "CSS style sheet",
        "js" | "mjs" => "JavaScript",
        "csv" => "CSV",
        "log" => "Log File",
        "zip" => "ZIP archive",
        "png" => "PNG image",
        "jpg" | "jpeg" => "JPEG image",
        "gif" => "GIF image",
        "webp" => "WebP Image",
        "svg" => "SVG image",
        "heic" => "HEIF Image",
        "tif" | "tiff" => "TIFF image",
        "bmp" => "Windows BMP image",
        "avif" => "AV1 Image File Format",
        "mp4" => "MPEG-4 movie",
        "m4v" => "Apple MPEG-4 movie",
        "mov" | "qt" => "QuickTime movie",
        "webm" => "WebM Media",
        "ogv" => "Ogg video",
        "ts" => "MPEG-2 Transport Stream",
        "m2ts" | "mts" => "AVCHD MPEG-2 Transport Stream",
        "mp3" => "MP3 audio",
        "wav" => "Waveform audio",
        "m4a" => "Apple MPEG-4 audio",
        "aac" | "adts" => "AAC audio",
        "flac" => "FLAC audio",
        "ogg" | "opus" => "Ogg Audio",
        "glb" => "GL Transmission Format - Binary",
        _ => "Document",
    }
}

/// File system callbacks off macOS: only kind names. There are no tags or file index; the core
/// works without them. Each platform adds its own trash (`windows.rs`, `linux.rs`).
pub fn file_system() -> northroom_core::ffi::NrFileSystem {
    northroom_core::ffi::NrFileSystem {
        describe_kind: Some(describe_kind),
        ..northroom_core::ffi::NrFileSystem::NONE
    }
}

/// Finder's kind name for `path`.
extern "C" fn describe_kind(path: *const c_char, out: *mut u8, capacity: usize) -> usize {
    // SAFETY: the core passes a NUL-terminated path.
    let path = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    let extension = std::path::Path::new(path.as_ref())
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_default();
    hand_over(kind_of_extension(&extension), out, capacity)
}

/// Copies up to `capacity` bytes of `text` to `out` and returns how many were written.
pub(super) fn hand_over(text: &str, out: *mut u8, capacity: usize) -> usize {
    let bytes = &text.as_bytes()[..text.len().min(capacity)];
    // SAFETY: the core passes room for `capacity` bytes at `out`.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    bytes.len()
}

/// `NrTrash` result when the item was not moved.
pub(super) const TRASH_REFUSED: i64 = -1;
/// `NrTrash` result when the item was trashed but its new path is unknown.
pub(super) const TRASH_LANDED_UNKNOWN: i64 = 0;

/// Writes a trashed item's new path into `landed` (`capacity` bytes) and returns its length, or
/// `TRASH_LANDED_UNKNOWN` when it isn't UTF-8 or doesn't fit, since a truncated path would name
/// the wrong file.
#[cfg(unix)]
pub(super) fn hand_over_landed(location: &Path, landed: *mut u8, capacity: usize) -> i64 {
    match location.to_str() {
        Some(text) if text.len() <= capacity => hand_over(text, landed, capacity) as i64,
        _ => TRASH_LANDED_UNKNOWN,
    }
}

/// Groups `paths` by parent folder, in first-seen order, for the file manager to show one window
/// per folder. Paths with no parent (a disk root) are dropped.
pub(super) fn group_by_folder(paths: &[PathBuf]) -> Vec<(&Path, Vec<&Path>)> {
    let mut folders: Vec<(&Path, Vec<&Path>)> = Vec::new();
    for path in paths {
        let Some(folder) = path.parent() else {
            continue;
        };
        match folders.iter_mut().find(|(known, _)| *known == folder) {
            Some((_, items)) => items.push(path),
            None => folders.push((folder, vec![path])),
        }
    }
    folders
}

/// `path` with backslash separators. Core paths may use forward slashes, which shell calls (the
/// Recycle Bin, storage lookups) reject.
#[cfg(any(test, target_os = "windows"))]
pub(super) fn windows_path_text(path: &Path) -> String {
    path.to_string_lossy().replace('/', "\\")
}

/// Size of a `DROPFILES` header: `pFiles`, `pt` (two values), `fNC` and `fWide`, 4 bytes each.
#[cfg(any(test, target_os = "windows"))]
pub(super) const DROP_FILES_HEADER_BYTES: usize = 20;

/// A `CF_HDROP` clipboard block as Explorer writes it: a `DROPFILES` header with `fWide` set,
/// then each UTF-16 name NUL-terminated, then a final NUL.
#[cfg(any(test, target_os = "windows"))]
pub(super) fn drop_files_block(names: &[String]) -> Vec<u8> {
    const WIDE: u32 = 1;
    let header = [DROP_FILES_HEADER_BYTES as u32, 0, 0, 0, WIDE];
    let list = names
        .iter()
        .flat_map(|name| name.encode_utf16().chain([0]))
        .chain([0]);
    header
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .chain(list.flat_map(u16::to_le_bytes))
        .collect()
}

/// A freedesktop.org trash: its folder (`files/` holds items, `info/` their `.trashinfo`
/// records) and the mount top that relative `Path=` values resolve against. The home trash has
/// none, since its records use absolute paths.
#[cfg(unix)]
pub(super) struct TrashCan {
    pub folder: PathBuf,
    pub top: Option<PathBuf>,
}

/// A `.trashinfo` record: the original path, and the deletion time in local time as
/// "2026-11-17T09:26:00", which sorts as text.
#[cfg(unix)]
#[derive(Debug, PartialEq)]
struct TrashInfo {
    original: PathBuf,
    deleted: String,
}

#[cfg(unix)]
impl TrashInfo {
    /// Parses `Path=` (percent-encoded) and `DeletionDate=` from the `[Trash Info]` group.
    fn parse(text: &str) -> Option<Self> {
        let (mut original, mut deleted, mut in_group) = (None, None, false);
        for line in text.lines() {
            if line.starts_with('[') {
                in_group = line == "[Trash Info]";
                continue;
            }
            if !in_group {
                continue;
            }
            if let Some(path) = line.strip_prefix("Path=") {
                original = percent_decoded(path);
            } else if let Some(date) = line.strip_prefix("DeletionDate=") {
                deleted = Some(date.to_owned());
            }
        }
        let original = <std::ffi::OsString as std::os::unix::ffi::OsStringExt>::from_vec(original?);
        Some(Self {
            original: PathBuf::from(original),
            deleted: deleted?,
        })
    }
}

/// Percent-decodes `text` to bytes; `None` when a `%` isn't followed by two hex digits.
#[cfg(unix)]
fn percent_decoded(text: &str) -> Option<Vec<u8>> {
    let hex = |digit: Option<&u8>| char::from(*digit?).to_digit(16);
    let mut bytes = text.as_bytes().iter();
    let mut decoded = Vec::with_capacity(text.len());
    while let Some(&byte) = bytes.next() {
        if byte == b'%' {
            let (high, low) = (hex(bytes.next())?, hex(bytes.next())?);
            decoded.push((high * 16 + low) as u8);
        } else {
            decoded.push(byte);
        }
    }
    Some(decoded)
}

/// The top of `folder`'s mount: its highest ancestor on the same device, where GIO looks for
/// `.Trash-<uid>`.
#[cfg(unix)]
pub(super) fn mount_top(folder: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;
    let device = std::fs::metadata(folder).ok()?.dev();
    let on_device = |path: &&Path| std::fs::metadata(path).is_ok_and(|meta| meta.dev() == device);
    folder
        .ancestors()
        .take_while(on_device)
        .last()
        .map(Path::to_path_buf)
}

/// Where `original` ended up in `cans`: the item under `files/` whose record names it and is
/// newest (latest `DeletionDate`, then latest record for ties within a second). GIO names items
/// "name.ext", "name.2.ext" and so on, so only records starting with the name are read.
#[cfg(unix)]
pub(super) fn find_trashed(original: &Path, cans: &[TrashCan]) -> Option<PathBuf> {
    let name = original.file_name()?.as_encoded_bytes();
    let stem = name.split(|&byte| byte == b'.').next().unwrap_or(name);
    cans.iter()
        .flat_map(|can| trashed_in(can, original, stem))
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, item)| item)
}

/// Items in `can` whose records name `original`, with their deletion date and record mtime.
#[cfg(unix)]
fn trashed_in(
    can: &TrashCan,
    original: &Path,
    stem: &[u8],
) -> Vec<((String, std::time::SystemTime), PathBuf)> {
    let Ok(records) = std::fs::read_dir(can.folder.join("info")) else {
        return Vec::new();
    };
    records
        .filter_map(Result::ok)
        .filter_map(|record| {
            let path = record.path();
            let item = path.file_stem()?;
            let is_record = path.extension()? == "trashinfo";
            (is_record && item.as_encoded_bytes().starts_with(stem)).then_some(())?;
            let info = TrashInfo::parse(&std::fs::read_to_string(&path).ok()?)?;
            let named = match &can.top {
                Some(top) => top.join(&info.original),
                None => info.original,
            };
            (named == original).then_some(())?;
            let written = record.metadata().ok()?.modified().ok()?;
            Some(((info.deleted, written), can.folder.join("files").join(item)))
        })
        .collect()
}

/// Image pixels in the window's colour space, sRGB here (`crate::display`): BGRA bytes,
/// straight alpha as GPUI expects, top row first.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// The image at `path`, rotated per its orientation, at most `pixels` on its long side, and
/// converted to sRGB from its matrix-and-curve profile. Unknown or missing profiles are treated
/// as sRGB.
#[cfg(not(target_os = "windows"))]
pub fn image_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let orientation = decoder.orientation().ok()?;
    let profile = decoder.icc_profile().ok().flatten();
    let mut picture = image::DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    let (width, height) = (picture.width(), picture.height());
    let long = width.max(height);
    if long > pixels {
        let scale = f64::from(pixels) / f64::from(long);
        let size = |side: u32| ((f64::from(side) * scale).round() as u32).max(1);
        picture = picture.thumbnail_exact(size(width), size(height));
    }
    let rgba = picture.into_rgba8();
    let (width, height) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    if let Some(profile) = profile {
        // Leave the image unchanged when the profile can't be read.
        let _ = colour::to_srgb(&mut bgra, &profile);
    }
    for pixel in bgra.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(Pixels {
        width,
        height,
        bgra,
    })
}

/// Windows and Linux add no system menu items (`mac::added_items`).
pub fn added_items(_index: usize, _ours: usize) -> Vec<String> {
    Vec::new()
}

/// Edit ▸ Speech does nothing off macOS.
pub fn start_speaking(_: &str) {
    eprintln!("speech\treading aloud is not on this platform yet");
}

pub fn stop_speaking() {}

/// No eyedropper off macOS: logs `colour_picker::EYEDROPPER_NOT_HERE` and never calls `picked`.
pub fn sample_colour(_picked: impl FnOnce(Option<[f64; 3]>) + 'static) {
    eprintln!(
        "picker\t{}",
        northroom_core::colour_picker::EYEDROPPER_NOT_HERE
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_named_in_finders_words_whatever_the_case_of_its_extension() {
        assert_eq!(kind_of_extension("MP4"), "MPEG-4 movie");
        assert_eq!(kind_of_extension("md"), "Markdown Document");
        assert_eq!(kind_of_extension("wav"), "Waveform audio");
        assert_eq!(kind_of_extension(""), "Document");
        assert_eq!(kind_of_extension("nterm"), "Document");
    }

    #[test]
    fn files_are_shown_a_window_a_folder_in_the_order_they_were_named() {
        let paths: Vec<PathBuf> = ["/a/one.png", "/b/two.png", "/a/three.png", "/"]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let grouped = group_by_folder(&paths);
        assert_eq!(
            grouped,
            vec![
                (
                    Path::new("/a"),
                    vec![Path::new("/a/one.png"), Path::new("/a/three.png")]
                ),
                (Path::new("/b"), vec![Path::new("/b/two.png")]),
            ],
            "the root has no folder to be shown in"
        );
    }

    #[test]
    fn a_path_is_written_with_windows_separators() {
        assert_eq!(
            windows_path_text(Path::new("C:\\Users\\me/Northroom/clip.mp4")),
            "C:\\Users\\me\\Northroom\\clip.mp4"
        );
    }

    #[test]
    fn the_clipboards_file_block_is_a_wide_dropfiles_header_then_the_names() {
        let names = ["C:\\a.txt".to_owned(), "D:\\é 🎬".to_owned()];
        let block = drop_files_block(&names);
        let header: Vec<u8> = [20u32, 0, 0, 0, 1]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        assert_eq!(block[..DROP_FILES_HEADER_BYTES], header[..]);
        let (units, rest) = block[DROP_FILES_HEADER_BYTES..].as_chunks::<2>();
        assert!(rest.is_empty(), "the names are whole UTF-16 units");
        let units: Vec<u16> = units.iter().map(|&unit| u16::from_le_bytes(unit)).collect();
        let expected: Vec<u16> = "C:\\a.txt\0D:\\é 🎬\0\0".encode_utf16().collect();
        assert_eq!(
            units, expected,
            "each name ends in a NUL, the list in another"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_trash_record_is_read_with_its_path_unescaped() {
        let record = "[Trash Info]\nPath=/home/me/Northroom/a%20clip%E2%9C%93.mp4\nDeletionDate=2026-11-17T09:26:00\n";
        assert_eq!(
            TrashInfo::parse(record),
            Some(TrashInfo {
                original: PathBuf::from("/home/me/Northroom/a clip✓.mp4"),
                deleted: "2026-11-17T09:26:00".to_owned(),
            })
        );
        let other_group = "[Desktop Entry]\nPath=/a\nDeletionDate=2026-11-17T09:26:00\n";
        assert_eq!(TrashInfo::parse(other_group), None);
        let broken_escape = "[Trash Info]\nPath=/a%2\nDeletionDate=2026-11-17T09:26:00\n";
        assert_eq!(TrashInfo::parse(broken_escape), None);
        assert_eq!(TrashInfo::parse("[Trash Info]\nPath=/a\n"), None);
    }

    #[cfg(unix)]
    /// A per-test scratch folder, removed when the test ends.
    struct Scratch(PathBuf);

    #[cfg(unix)]
    impl Scratch {
        fn new(name: &str) -> Self {
            let folder =
                std::env::temp_dir().join(format!("nr-portable-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).expect("a scratch folder");
            Self(folder)
        }
    }

    #[cfg(unix)]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    /// Puts an item named `item` in the trash at `can` with a record naming `path`.
    fn trash_item(can: &Path, item: &str, path: &str, deleted: &str) {
        for folder in ["files", "info"] {
            std::fs::create_dir_all(can.join(folder)).expect("the trash's folders");
        }
        std::fs::write(can.join("files").join(item), b"").expect("the item");
        let record = format!("[Trash Info]\nPath={path}\nDeletionDate={deleted}\n");
        std::fs::write(can.join("info").join(format!("{item}.trashinfo")), record)
            .expect("its record");
    }

    #[cfg(unix)]
    #[test]
    fn a_trashed_file_is_found_by_its_newest_record() {
        let scratch = Scratch::new("trash-home");
        let home = scratch.0.join("Trash");
        let original = Path::new("/home/me/Northroom/clip.mp4");
        trash_item(
            &home,
            "clip.mp4",
            "/home/me/Northroom/clip.mp4",
            "2026-11-16T10:00:00",
        );
        trash_item(
            &home,
            "clip.2.mp4",
            "/home/me/Northroom/clip.mp4",
            "2026-11-17T09:26:00",
        );
        trash_item(
            &home,
            "clip.3.mp4",
            "/home/me/Other/clip.mp4",
            "2026-11-17T09:30:00",
        );
        trash_item(
            &home,
            "clipping.mp4",
            "/home/me/Northroom/clip.mp4",
            "2026-11-17T09:00:00",
        );
        let cans = [TrashCan {
            folder: home.clone(),
            top: None,
        }];
        assert_eq!(
            find_trashed(original, &cans),
            Some(home.join("files").join("clip.2.mp4")),
            "the newest record of this path, not another folder's file of the same name"
        );
        assert_eq!(
            find_trashed(Path::new("/home/me/Northroom/gone.mp4"), &cans),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_mounts_trash_names_its_items_from_the_mounts_top() {
        let scratch = Scratch::new("trash-mount");
        let top = scratch.0.join("media");
        let mount_can = top.join(".Trash-1000");
        trash_item(
            &mount_can,
            "take one.wav",
            "Sound/take%20one.wav",
            "2026-11-17T09:26:00",
        );
        let cans = [
            TrashCan {
                folder: scratch.0.join("home-Trash"),
                top: None,
            },
            TrashCan {
                folder: mount_can.clone(),
                top: Some(top.clone()),
            },
        ];
        assert_eq!(
            find_trashed(&top.join("Sound/take one.wav"), &cans),
            Some(mount_can.join("files").join("take one.wav"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_folders_mount_top_is_its_farthest_ancestor_on_the_same_device() {
        use std::os::unix::fs::MetadataExt;
        let scratch = Scratch::new("mount-top");
        let top = mount_top(&scratch.0).expect("a folder is on a mount");
        let device = |path: &Path| std::fs::metadata(path).expect("a folder").dev();
        assert!(scratch.0.starts_with(&top));
        assert_eq!(device(&top), device(&scratch.0));
        if let Some(above) = top.parent() {
            assert_ne!(device(above), device(&top), "the top is the farthest one");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_landed_path_is_handed_over_whole_or_not_at_all() {
        let mut room = [0u8; 16];
        let path = Path::new("/trash/files/a");
        assert_eq!(hand_over_landed(path, room.as_mut_ptr(), room.len()), 14);
        assert_eq!(&room[..14], b"/trash/files/a");
        let mut small = [0u8; 4];
        assert_eq!(
            hand_over_landed(path, small.as_mut_ptr(), small.len()),
            TRASH_LANDED_UNKNOWN
        );
    }
}
