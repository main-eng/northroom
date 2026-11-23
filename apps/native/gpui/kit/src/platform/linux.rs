//! Linux platform services: window capture from the X server, the desktop trash (`trash`) and
//! revealing files (`reveal`), plus the shared non-macOS pieces in `portable.rs`. There are no
//! Finder tags or file index; the core works without them.
//!
//! There is no `share` (Linux has no share sheet) and no `write_files`: GPUI's Linux clipboard
//! is text only, so copied files go on it as paths, one per line (`clipboard.rs`).

use std::path::{Path, PathBuf};

pub use super::portable::{
    Pixels, added_items, image_thumbnail, sample_colour, start_speaking, stop_speaking,
};

/// File system callbacks passed to the core (`nr_drive_start`): kind names (`portable.rs`) and
/// the desktop trash.
pub fn file_system() -> northroom_core::ffi::NrFileSystem {
    northroom_core::ffi::NrFileSystem {
        trash: Some(trash),
        ..super::portable::file_system()
    }
}

/// The path the core passes.
///
/// # Safety
/// `path` is a NUL-terminated string that outlives the answer.
unsafe fn core_path<'a>(path: *const std::ffi::c_char) -> &'a Path {
    // SAFETY: per the caller.
    let bytes = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes();
    Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(bytes))
}

/// Moves a file or folder to the desktop trash (GIO's `g_file_trash`, per the freedesktop.org
/// Trash spec, including a mount's `.Trash-<uid>`) and writes the new path into `landed`. On a
/// mount with no trash it fails and leaves the file; it never falls back to deleting.
extern "C" fn trash(path: *const std::ffi::c_char, landed: *mut u8, capacity: usize) -> i64 {
    use super::portable::{TRASH_LANDED_UNKNOWN, TRASH_REFUSED, find_trashed, hand_over_landed};
    use gio::prelude::FileExt;
    if path.is_null() {
        return TRASH_REFUSED;
    }
    // SAFETY: the core passes a NUL-terminated path.
    let path = unsafe { core_path(path) };
    if let Err(error) = gio::File::for_path(path).trash(gio::Cancellable::NONE) {
        eprintln!("the trash refused {}: {error}", path.display());
        return TRASH_REFUSED;
    }
    match find_trashed(path, &trash_cans(path)) {
        Some(location) => hand_over_landed(&location, landed, capacity),
        None => {
            eprintln!(
                "the trash took {} but its record was not found",
                path.display()
            );
            TRASH_LANDED_UNKNOWN
        }
    }
}

/// Trash folders GIO may have moved `trashed` into: the home trash (`$XDG_DATA_HOME/Trash`),
/// and the mount's top-level `.Trash/<uid>` (shared) and `.Trash-<uid>`.
fn trash_cans(trashed: &Path) -> Vec<super::portable::TrashCan> {
    use super::portable::{TrashCan, mount_top};
    let home = TrashCan {
        folder: gio::glib::user_data_dir().join("Trash"),
        top: None,
    };
    let Some(top) = trashed.parent().and_then(mount_top) else {
        return vec![home];
    };
    // SAFETY: always succeeds.
    let user = unsafe { libc::getuid() };
    vec![
        home,
        TrashCan {
            folder: top.join(".Trash").join(user.to_string()),
            top: Some(top.clone()),
        },
        TrashCan {
            folder: top.join(format!(".Trash-{user}")),
            top: Some(top),
        },
    ]
}

/// Bus name, object path and interface of `org.freedesktop.FileManager1`.
const FILE_MANAGER: &str = "org.freedesktop.FileManager1";
const FILE_MANAGER_OBJECT: &str = "/org/freedesktop/FileManager1";
/// Timeout for the file manager to reply, including its startup.
const FILE_MANAGER_TIMEOUT_MS: i32 = 10_000;

/// Selects `paths` in the file manager via `org.freedesktop.FileManager1.ShowItems` (Files,
/// Dolphin, Nemo and others). Runs off the UI thread because the bus may need to start it.
/// Without a reply, opens each parent folder in its default app instead.
pub fn reveal(paths: &[PathBuf]) {
    let paths = paths.to_vec();
    std::thread::spawn(move || {
        if let Err(error) = show_items(&paths) {
            eprintln!(
                "reveal\tthe file manager did not show the files ({error}): opening their folders"
            );
            open_folders(&paths);
        }
    });
}

fn show_items(paths: &[PathBuf]) -> Result<(), gio::glib::Error> {
    use gio::prelude::{FileExt, ToVariant};
    let uris: Vec<String> = paths
        .iter()
        .map(|path| gio::File::for_path(path).uri().into())
        .collect();
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    // No startup notification; focus is up to the file manager.
    let startup_id = "";
    bus.call_sync(
        Some(FILE_MANAGER),
        FILE_MANAGER_OBJECT,
        FILE_MANAGER,
        "ShowItems",
        Some(&(uris, startup_id).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        FILE_MANAGER_TIMEOUT_MS,
        gio::Cancellable::NONE,
    )?;
    Ok(())
}

/// Opens each parent folder of `paths` in the default folder app.
fn open_folders(paths: &[PathBuf]) {
    use gio::prelude::FileExt;
    for (folder, _) in super::portable::group_by_folder(paths) {
        let uri = gio::File::for_path(folder).uri();
        if let Err(error) = gio::AppInfo::launch_default_for_uri(&uri, gio::AppLaunchContext::NONE)
        {
            eprintln!("reveal\t{} was not opened: {error}", folder.display());
        }
    }
}

/// The parent folders for the app's own, from the XDG base directories: data, state (for logs)
/// and cache (`nr_paths_open`).
pub fn app_folders() -> (String, String, String) {
    let home = std::env::var("HOME").unwrap_or_default();
    let base = |variable: &str, under_home: &str| {
        std::env::var(variable)
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("{home}/{under_home}"))
    };
    (
        base("XDG_DATA_HOME", ".local/share"),
        base("XDG_STATE_HOME", ".local/state"),
        base("XDG_CACHE_HOME", ".cache"),
    )
}

/// The frame `seconds` into the video at `path`, at most `pixels` on its long side and rotated
/// for display. The core decodes it with GStreamer (`northroom_core::ffi::video_still`).
pub fn video_still(path: &str, seconds: f64, pixels: u32) -> Option<Pixels> {
    let still = northroom_core::ffi::video_still(path, seconds, pixels)?;
    Some(Pixels {
        width: still.width,
        height: still.height,
        bgra: still.bgra,
    })
}

/// No system thumbnails on Linux, so such files keep their glyph.
pub fn system_thumbnail(_path: &str, _pixels: u32) -> Option<Pixels> {
    None
}

/// The kernel release (`uname`) as major, minor and patch, for the diagnostic log.
pub fn system_version() -> (u32, u32, u32) {
    // SAFETY: `uname` fills the structure it is given; its fields are NUL-terminated.
    let release = unsafe {
        let mut names: libc::utsname = std::mem::zeroed();
        assert_eq!(libc::uname(&mut names), 0, "the system says its release");
        std::ffi::CStr::from_ptr(names.release.as_ptr())
            .to_string_lossy()
            .into_owned()
    };
    super::system::version_parts(&release)
}

/// The desktop's reduce-motion setting isn't read, so spinners always turn.
pub fn reduce_motion() -> bool {
    false
}

/// Turns font smoothing off so each pixel uses the glyph's raw coverage. Otherwise GPUI's Linux
/// renderer corrects coverage for gamma 1.8 (light-on-dark glyphs come out about a third heavier
/// at half coverage) and thickens dark-on-light glyphs (enhanced contrast). It reads both from
/// the environment once, when a window's renderer is created, so set them before anything starts.
///
/// # Safety
/// No other thread may be running, because this changes the environment.
pub unsafe fn draw_glyphs_unsmoothed() {
    // SAFETY: per the caller.
    unsafe {
        // Gamma for coverage correction; 1 means none.
        std::env::set_var("ZED_FONTS_GAMMA", "1");
        std::env::set_var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST", "0");
    }
}

/// Saves the window as the X server shows it to `path` (PNG). GPUI on Linux has no
/// `render_to_image`, so this reads back the presented frame, which is never covered on CI's
/// Xvfb.
pub fn save_window_picture(window: &gpui::Window, path: &str) -> Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{ConnectionExt, ImageFormat, ImageOrder};
    // The raw handle (`HasWindowHandle`), not GPUI's own `Window::window_handle`.
    let handle = HasWindowHandle::window_handle(window).map_err(|error| error.to_string())?;
    let id = match handle.as_raw() {
        RawWindowHandle::Xcb(window) => window.window.get(),
        RawWindowHandle::Xlib(window) => window.window as u32,
        other => return Err(format!("not an X11 window: {other:?}")),
    };
    let (connection, _) = x11rb::connect(None).map_err(|error| error.to_string())?;
    let failed = |error: &dyn std::fmt::Display| error.to_string();
    let geometry = connection
        .get_geometry(id)
        .map_err(|error| failed(&error))?
        .reply()
        .map_err(|error| failed(&error))?;
    let image = connection
        .get_image(
            ImageFormat::Z_PIXMAP,
            id,
            0,
            0,
            geometry.width,
            geometry.height,
            !0,
        )
        .map_err(|error| failed(&error))?
        .reply()
        .map_err(|error| failed(&error))?;
    let setup = connection.setup();
    let bits = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == image.depth)
        .map(|format| format.bits_per_pixel);
    if bits != Some(32) || setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err(format!(
            "the X server's pictures are {bits:?} bits a pixel, {:?}: only 32-bit little-endian ones are read",
            setup.image_byte_order
        ));
    }
    // BGRX per pixel; the window is opaque.
    let (pixels, _) = image.data.as_chunks::<4>();
    let rgba: Vec<u8> = pixels
        .iter()
        .flat_map(|&[blue, green, red, _]| [red, green, blue, 255])
        .collect();
    image::RgbaImage::from_raw(u32::from(geometry.width), u32::from(geometry.height), rgba)
        .ok_or("the X server sent fewer pixels than the window has")?
        .save(path)
        .map_err(|error| error.to_string())
}
