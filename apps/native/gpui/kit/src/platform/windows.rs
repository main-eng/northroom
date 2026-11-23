//! Windows platform services: the OS version, the tray icon (`tray`), full-size check windows,
//! image decoding through WIC, the Recycle Bin (`trash`), Explorer integration (`explorer`),
//! the share sheet (`share`), plus the shared non-macOS pieces in `portable.rs`. There are no
//! Finder tags or file index; the core works without them.

use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;

mod drag;
mod explorer;
mod share;
mod signature;
mod thumbnail;
mod tray;

pub use drag::drag_files_out;
pub use explorer::{reveal, write_files};
pub use share::share;
pub use signature::signature;
pub use thumbnail::system_thumbnail;
pub use tray::{Tray, show_tray};

pub use super::portable::{Pixels, added_items, sample_colour, start_speaking, stop_speaking};

/// File system callbacks passed to the core (`nr_drive_start`): kind names (`portable.rs`) and
/// the Recycle Bin.
pub fn file_system() -> northroom_core::ffi::NrFileSystem {
    northroom_core::ffi::NrFileSystem {
        trash: Some(trash),
        ..super::portable::file_system()
    }
}

/// Moves a file or folder to the Recycle Bin without confirmation, like Explorer's Delete.
/// Items the Recycle Bin can't take (network shares, oversized files) would otherwise be
/// deleted silently, so `FOF_WANTNUKEWARNING` makes Windows ask first; "no" leaves the file.
///
/// Returns `TRASH_LANDED_UNKNOWN`, so the core offers no undo. The Recycle Bin stores a renamed
/// file plus a record (`$R…` and `$I…` in `$Recycle.Bin\<user>`) and `SHFileOperationW`
/// reports neither. Moving the file back alone would orphan the record; Explorer's Restore is
/// the way back.
extern "C" fn trash(path: *const std::ffi::c_char, _landed: *mut u8, _capacity: usize) -> i64 {
    use super::portable::{TRASH_LANDED_UNKNOWN, TRASH_REFUSED};
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    if path.is_null() {
        return TRASH_REFUSED;
    }
    // SAFETY: the core passes a NUL-terminated path.
    let path = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy();
    // Each path is NUL-terminated, and the list ends with another NUL.
    let from: Vec<u16> = super::portable::windows_path_text(std::path::Path::new(path.as_ref()))
        .encode_utf16()
        .chain([0, 0])
        .collect();
    let flags = FOF_ALLOWUNDO.0
        | FOF_NOCONFIRMATION.0
        | FOF_WANTNUKEWARNING.0
        | FOF_NOERRORUI.0
        | FOF_SILENT.0;
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: windows::core::PCWSTR(from.as_ptr()),
        fFlags: flags as u16,
        ..Default::default()
    };
    // SAFETY: `operation` and the path list it points to outlive the call.
    let result = unsafe { SHFileOperationW(&mut operation) };
    if result == 0 && !operation.fAnyOperationsAborted.as_bool() {
        TRASH_LANDED_UNKNOWN
    } else {
        TRASH_REFUSED
    }
}

/// The Win32 window behind `window`.
fn window_handle(window: &gpui::Window) -> windows::Win32::Foundation::HWND {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // The raw handle (`HasWindowHandle`), not GPUI's own `Window::window_handle`.
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(window)) => {
            windows::Win32::Foundation::HWND(window.hwnd.get() as _)
        }
        _ => panic!("a GPUI window on Windows has a Win32 window"),
    }
}

/// The parent folders for the app's own: roaming app data for its files, local app data for
/// logs and caches (`nr_paths_open`).
pub fn app_folders() -> (String, String, String) {
    let roaming = std::env::var("APPDATA").unwrap_or_default();
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    (roaming, local.clone(), local)
}

/// The frame `seconds` into the video at `path`, at most `pixels` on its long side and rotated
/// for display. The core decodes it with Media Foundation (`northroom_core::ffi::video_still`).
pub fn video_still(path: &str, seconds: f64, pixels: u32) -> Option<Pixels> {
    let still = northroom_core::ffi::video_still(path, seconds, pixels)?;
    Some(Pixels {
        width: still.width,
        height: still.height,
        bgra: still.bgra,
    })
}

/// The image at `path`, rotated per its orientation, at most `pixels` on its long side, in sRGB.
/// The core decodes it with WIC (`northroom_core::ffi::picture_thumbnail`), so HEIC and AVIF
/// work when Windows has their extensions.
pub fn image_thumbnail(path: &str, pixels: u32) -> Option<Pixels> {
    let picture = northroom_core::ffi::picture_thumbnail(path, pixels)?;
    Some(Pixels {
        width: picture.width,
        height: picture.height,
        bgra: picture.bgra,
    })
}

/// The OS major, minor and build number for the diagnostic log, from `RtlGetVersion`. Unlike
/// `GetVersionEx`, it doesn't report an older Windows to apps without a manifest entry.
pub fn system_version() -> (u32, u32, u32) {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    // SAFETY: the struct's size field is correct and the call fills it.
    let read = unsafe { RtlGetVersion(&mut version) };
    assert!(read.is_ok(), "the system says its version");
    (
        version.dwMajorVersion,
        version.dwMinorVersion,
        version.dwBuildNumber,
    )
}

/// Windows' "Show animations" setting isn't read, so spinners always turn.
pub fn reduce_motion() -> bool {
    false
}

/// Sizes a check's window to `width` by `height` points at 2x on any display, as on macOS.
///
/// A small display (Windows keeps framed windows inside it, and GPUI shrinks windows that don't
/// fit) or a 1x display (a CI runner's 1024 x 768 at 96 dpi) would render it smaller or coarser.
/// So the window drops its frame, which lets it keep any size, and receives `WM_DPICHANGED` for
/// 192 dpi, which GPUI handles by taking the new scale and suggested size. Checks read GPUI's
/// frames (`render_to_image`), so what the display shows doesn't matter. The window never
/// actually moves, so Windows never sees a display change.
///
/// GPUI handles the message in the window procedure, which updates the app, so this runs once
/// the app is free, soon after the caller returns.
pub fn draw_check_window_whole(
    handle: gpui::AnyWindowHandle,
    width: f64,
    height: f64,
    cx: &mut gpui::App,
) {
    use gpui::AppContext;
    use windows::Win32::Foundation::{LPARAM, RECT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_STYLE, GetWindowLongPtrW, GetWindowRect, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE,
        SWP_NOSIZE, SWP_NOZORDER, SendMessageW, SetWindowLongPtrW, SetWindowPos,
        USER_DEFAULT_SCREEN_DPI, WM_DPICHANGED, WS_OVERLAPPEDWINDOW, WS_POPUP,
    };
    /// Pixels per point on a Retina display.
    const SCALE: u32 = 2;
    cx.spawn(async move |cx| {
        let window = cx
            .update_window(handle, |_, window, _| window_handle(window))
            .expect("the check's window is open");
        let dpi = USER_DEFAULT_SCREEN_DPI * SCALE;
        // SAFETY: the app's window, on its own thread while GPUI holds no borrow of the app; the
        // suggested frame outlives the message, which is handled synchronously.
        unsafe {
            let style = GetWindowLongPtrW(window, GWL_STYLE);
            let unframed = (style & !(WS_OVERLAPPEDWINDOW.0 as isize)) | WS_POPUP.0 as isize;
            SetWindowLongPtrW(window, GWL_STYLE, unframed);
            SetWindowPos(
                window,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .expect("the check's window loses its frame");
            let mut frame = RECT::default();
            GetWindowRect(window, &mut frame).expect("the check's window has a frame");
            let suggested = RECT {
                left: frame.left,
                top: frame.top,
                right: frame.left + (width * f64::from(SCALE)) as i32,
                bottom: frame.top + (height * f64::from(SCALE)) as i32,
            };
            SendMessageW(
                window,
                WM_DPICHANGED,
                Some(WPARAM((dpi | dpi << 16) as usize)),
                Some(LPARAM((&raw const suggested) as isize)),
            );
        }
    })
    .detach();
}
