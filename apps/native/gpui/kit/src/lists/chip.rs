//! The chip that follows the pointer during a file drag: panel colour, 1 px border, 6 px
//! corners. The core supplies its label (`nr_drag_label`), size (`drag_chip`) and pointer
//! offset (`nr_drag_chip_pointer`).
//!
//! There is at most one drag in the window, so its state lives here. The source table calls
//! `begin`, tables under the pointer ask `carried`, the table under the release ends it, and
//! the window draws the chip last (`paint`). A drag that leaves the window becomes a platform
//! drag other apps can take (`carry_out`).

use super::thumbs;
use crate::controls::Drawn;
use crate::core::{c, frame, inset, text};
use crate::draw::{rounded, stroke};
use crate::theme::{Token, color};
use gpui::{App, Bounds, Pixels, Point, RenderImage, Window};
use northroom_core::app_ffi::{drag_chip, nr_drag_chip_pointer};
use northroom_core::ffi::{nr_drag_label, nr_entry_path};
use northroom_core::window::screen_layout::DragChip;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::Arc;

/// The chip for files titled `titles`, naming the first.
pub fn chip_of(titles: &[String]) -> DragChip {
    let first = c(titles.first().map_or("", String::as_str));
    // SAFETY: a NUL-terminated title.
    let label = text(&unsafe { nr_drag_label(titles.len(), first.as_ptr()) });
    drag_chip(&label)
}

/// Where `chip` sits while the pointer is at `at`.
pub fn chip_frame(chip: &DragChip, at: Point<Pixels>) -> Bounds<Pixels> {
    let pointer = nr_drag_chip_pointer() as f32;
    frame(
        f32::from(at.x) - pointer,
        f32::from(at.y) - pointer,
        chip.width as f32,
        chip.height as f32,
    )
}

/// Draws `chip` with the pointer at `at`.
fn paint_chip(chip: &DragChip, window: &mut Window, cx: &mut App, at: Point<Pixels>) {
    let bounds = chip_frame(chip, at);
    let surface = inset(bounds, 0.5, 0.5);
    // Fill the same path the border strokes, as `NSBezierPath` does, so the border's outer
    // half overlaps what is under the chip.
    rounded(window, surface, color(Token::Panel), 5.5);
    stroke(window, surface, color(Token::Border), 5.5, 1.0);
    chip.words.draw(window, cx, bounds.origin, None);
}

/// The `Drag::from` value for a drag that left the board.
pub const BOARD: i64 = -2;

/// A drag of the drive's files under way inside the window.
#[derive(Clone)]
pub struct Drag {
    /// The source table: -1 the list, `BOARD` the board, otherwise a column number.
    pub from: i64,
    pub ids: Vec<String>,
    pub chip: DragChip,
    pub at: Point<Pixels>,
    /// The files' pictures at pickup, one per file. Pictures of cards leaving the board are
    /// released once no frame draws them.
    pictures: Vec<Option<Arc<RenderImage>>>,
    /// The files left as a platform drag (`carry_out`). The in-window drag ends once every
    /// table has heard the move that took them out.
    out: bool,
}

/// A file carried out of the window: its path and its picture, if it had one.
pub struct Carried {
    pub path: PathBuf,
    pub picture: Option<Arc<RenderImage>>,
}

thread_local! {
    static DRAG: RefCell<Option<Drag>> = const { RefCell::new(None) };
}

/// Files `ids` titled `titles` left table `from`, the pointer at `at`.
pub fn begin(from: i64, ids: Vec<String>, titles: &[String], at: Point<Pixels>) {
    let pictures = ids.iter().map(|id| thumbs::kept_picture(id)).collect();
    let drag = Drag {
        from,
        ids,
        chip: chip_of(titles),
        at,
        pictures,
        out: false,
    };
    DRAG.with(|kept| *kept.borrow_mut() = Some(drag));
}

/// Moves the drag to `at`. True when a drag is under way.
pub fn follow(at: Point<Pixels>) -> bool {
    DRAG.with(|kept| {
        kept.borrow_mut()
            .as_mut()
            .map(|drag| drag.at = at)
            .is_some()
    })
}

/// The ids the current drag carries, if any.
pub fn carried() -> Option<Vec<String>> {
    DRAG.with(|kept| kept.borrow().as_ref().map(|drag| drag.ids.clone()))
}

pub fn current() -> Option<Drag> {
    DRAG.with(|kept| kept.borrow().clone())
}

/// Ends the drag, dropped or not, and returns it.
pub fn end() -> Option<Drag> {
    DRAG.with(|kept| kept.borrow_mut().take())
}

/// Hands the drag to the platform once the pointer at `at` leaves the window, so other apps
/// (Finder, Mail, an upload field) can take the files. The in-window drag ends after every
/// table has heard this move and dropped its ring or stand-in. True once the drag has gone out,
/// on this call or an earlier one. Mac and Windows only: Linux GPUI has no drag source.
pub fn carry_out(at: Point<Pixels>, window: &mut Window, cx: &mut App) -> bool {
    let Some(drag) = current() else {
        return false;
    };
    if drag.out {
        return true;
    }
    if !CARRIES_OUT || Bounds::new(Point::default(), window.viewport_size()).contains(&at) {
        return false;
    }
    let files: Vec<Carried> = drag
        .ids
        .iter()
        .zip(drag.pictures)
        .filter_map(|(id, picture)| {
            let id = c(id);
            // SAFETY: a NUL-terminated id.
            let path = text(&unsafe { nr_entry_path(id.as_ptr()) });
            (!path.is_empty()).then(|| Carried {
                path: PathBuf::from(path),
                picture,
            })
        })
        .collect();
    if files.is_empty() || !drag_out(&files, window, cx) {
        return false;
    }
    DRAG.with(|kept| {
        if let Some(drag) = kept.borrow_mut().as_mut() {
            drag.out = true;
        }
    });
    window.defer(cx, |window, _| {
        end();
        window.refresh();
    });
    true
}

/// Whether the platform takes a drag of files out of the window (`drag_out`).
const CARRIES_OUT: bool = cfg!(any(target_os = "macos", target_os = "windows"));

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn drag_out(files: &[Carried], window: &mut Window, cx: &mut App) -> bool {
    crate::platform::drag_files_out(files, window, cx)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn drag_out(_: &[Carried], _: &mut Window, _: &mut App) -> bool {
    false
}

/// Draws the chip of the drag under way, if any, where the pointer is.
pub fn paint(window: &mut Window, cx: &mut App) {
    if let Some(drag) = current() {
        paint_chip(&drag.chip, window, cx, drag.at);
    }
}
