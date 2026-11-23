//! Platform services, one module per platform, each with the same calls: font tables, app
//! folders, file kind names, image and video thumbnails, the OS version, and files on the
//! clipboard (`write_files`), in the share sheet (`share`, not on Linux) and in the file manager
//! (`reveal`). macOS adds Finder tags and the file index. Code shared by Windows and Linux lives
//! in `portable`.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod mac;
// On macOS `portable` is built only for its tests.
#[cfg(any(test, not(target_os = "macos")))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod portable;
#[cfg(target_os = "windows")]
mod windows;
// Every copy and paste goes through this; unattended runs use a check's own clipboard.
mod clipboard;
mod system;
// The tray icon's menu, on platforms that have one.
#[cfg(any(target_os = "macos", target_os = "windows"))]
mod tray;

pub use clipboard::{
    clipboard_holds_files, read_clipboard, read_clipboard_files, write_clipboard,
    write_clipboard_files,
};
#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(target_os = "macos")]
pub use mac::*;
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use tray::{TrayMenu, TrayRow};
#[cfg(target_os = "windows")]
pub use windows::*;
