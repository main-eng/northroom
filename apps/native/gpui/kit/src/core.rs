//! The core's boundary in GPUI types. The core keeps its state behind its `nr_*` functions,
//! which are linked into this program; the shell calls them and converts their results here.

use gpui::{Bounds, Pixels, Point, point, px, size};
use northroom_core::ffi::{NrRect, NrText};
use std::ffi::CString;

/// Copies borrowed text the core handed out.
pub fn text(text: &NrText) -> String {
    // SAFETY: the core hands out `length` bytes of UTF-8 at `bytes`, alive until its next call.
    unsafe { text.to_owned_string() }
}

/// `text` as a NUL-terminated string for a boundary function.
pub fn c(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no NUL is left")
}

pub fn rect(rect: &NrRect) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(rect.x as f32), px(rect.y as f32)),
        size: size(px(rect.width as f32), px(rect.height as f32)),
    }
}

pub fn nr_rect(bounds: Bounds<Pixels>) -> NrRect {
    NrRect {
        x: f64::from(bounds.origin.x),
        y: f64::from(bounds.origin.y),
        width: f64::from(bounds.size.width),
        height: f64::from(bounds.size.height),
    }
}

pub fn empty_rect() -> NrRect {
    NrRect {
        x: 0.0,
        y: 0.0,
        width: 0.0,
        height: 0.0,
    }
}

/// A boundary struct for which all zeroes is a valid value: a `#[repr(C)]` struct of numbers,
/// flags, raw pointers and enums with a variant of 0, and of such structs.
///
/// # Safety
/// Implemented only for such a type: no reference, no `NonNull`, no function pointer.
pub unsafe trait Zeroed {}

// SAFETY: each is the core's `#[repr(C)]` struct of numbers, flags, raw pointers, `NrText`,
// `NrRect`, `NrColor` and enums whose first variant is 0 (`NrFileKind::Folder`,
// `NrDetailsShows::Glyph`, `NrChangesStatus::Added`, `NrChangesRowShows::Header`).
unsafe impl Zeroed for NrText {}
unsafe impl Zeroed for NrRect {}
unsafe impl Zeroed for northroom_core::changes::ffi::NrChangesFile {}
unsafe impl Zeroed for northroom_core::changes::ffi::NrChangesFrames {}
unsafe impl Zeroed for northroom_core::changes::ffi::NrChangesRow {}
unsafe impl Zeroed for northroom_core::ffi::NrDetails {}
unsafe impl Zeroed for northroom_core::ffi::NrFileRow {}
unsafe impl Zeroed for northroom_core::ffi::NrListRow {}
#[cfg(not(target_os = "macos"))]
unsafe impl Zeroed for northroom_core::ffi::NrPreviewPicture {}
unsafe impl Zeroed for northroom_core::ffi::NrSyntaxRun {}
unsafe impl Zeroed for northroom_core::ffi::NrTerminalFrame {}
unsafe impl Zeroed for northroom_core::menus::ffi::NrListPlaced {}
unsafe impl Zeroed for northroom_core::script::ffi::NrScriptPicture {}
unsafe impl Zeroed for northroom_core::script::ffi::NrScriptPress {}
unsafe impl Zeroed for northroom_core::window::ffi::NrTitleBar {}
unsafe impl Zeroed for northroom_core::window::ffi::NrTitleBarTab {}

/// A zeroed boundary struct for an `out` parameter to fill.
pub fn zeroed<T: Zeroed>() -> T {
    // SAFETY: all zeroes is a valid `T` (`Zeroed`), which the core then overwrites.
    unsafe { std::mem::zeroed() }
}

pub fn is_empty(bounds: &Bounds<Pixels>) -> bool {
    bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0)
}

pub fn contains(bounds: &Bounds<Pixels>, at: Point<Pixels>) -> bool {
    at.x >= bounds.origin.x
        && at.x < bounds.origin.x + bounds.size.width
        && at.y >= bounds.origin.y
        && at.y < bounds.origin.y + bounds.size.height
}

pub fn inset(bounds: Bounds<Pixels>, dx: f32, dy: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(bounds.origin.x + px(dx), bounds.origin.y + px(dy)),
        size: size(
            bounds.size.width - px(2.0 * dx),
            bounds.size.height - px(2.0 * dy),
        ),
    }
}

pub fn offset(bounds: Bounds<Pixels>, dx: f32, dy: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(bounds.origin.x + px(dx), bounds.origin.y + px(dy)),
        size: bounds.size,
    }
}

pub fn frame(x: f32, y: f32, width: f32, height: f32) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(x), px(y)),
        size: size(px(width), px(height)),
    }
}
