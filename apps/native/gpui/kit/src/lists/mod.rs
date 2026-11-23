//! The drive's lists: the list and its `header`, a folder's `column`, the shared `scroll`er,
//! the `rename` field, the command `palette` and the drag `chip`.
//!
//! The core owns rows, their order, pitch and layout (`nr_list_row_layout`,
//! `nr_column_row_layout`), and what presses and keys do to the selection (`nr_rows_press`,
//! `nr_rows_key`, `nr_rows_pick_up`, `nr_rows_selected`). Only visible rows are built and drawn.
//!
//! The technique is GPUI Kit's virtual list, reimplemented without its code (no notice owed): a
//! view paints its visible rows in one canvas from the row pitch and hit-tests the pointer with
//! the same arithmetic. A followed press is heard through the window's mouse listeners.

pub mod chip;
pub mod column;
pub mod header;
pub mod list;
pub mod palette;
pub mod rename;
pub mod rows;
pub mod scroll;
pub mod thumbs;
pub mod voiceover;

use crate::controls::input;
use crate::core::{frame, inset, text};
use crate::draw::{fill, oval, rounded};
use crate::icons::{self, Glyph};
use crate::nr::{NrDriveEdit, nr_drive_edit, nr_drive_paste};
use crate::theme::{self, CONTROL_RADIUS, Token, color, mixed};
use gpui::{
    App, Bounds, Corners, FocusHandle, Hsla, InteractiveElement, KeyBinding, MouseButton,
    MouseDownEvent, Pixels, Window, actions, px,
};
use northroom_core::ffi::{NrFileKind, NrFileRow, nr_terminal_look};
use northroom_core::window::ffi::{NrRows, nr_rows_selected};
use std::rc::Rc;

/// Whether a press opens a row or header menu: a right click, or a Control-left click.
pub(crate) fn asks_for_menu(event: &MouseDownEvent) -> bool {
    event.button == MouseButton::Right
        || (event.button == MouseButton::Left && event.modifiers.control)
}

/// What a row of the list or a column shows of its file (`NrFileRow`).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub title: String,
    pub kind: NrFileKind,
    /// The core's glyph for the file's kind, if any.
    pub glyph: Option<Glyph>,
    pub holds_files: bool,
    pub terminal_activity: u32,
    pub terminal_logo: String,
}

impl Item {
    pub fn of(row: &NrFileRow) -> Self {
        Self {
            id: text(&row.id),
            title: text(&row.title),
            kind: row.kind,
            glyph: icons::named(&text(&row.glyph)),
            holds_files: row.holds_files,
            terminal_activity: row.terminal_activity,
            terminal_logo: text(&row.terminal_logo),
        }
    }
}

/// The key context of the drive's file views: the list, a column and the board.
pub const CONTEXT: &str = "KitRows";

actions!(kit_rows, [Duplicate]);

/// The drive's key bindings in `CONTEXT`: ⌘C copy, ⌘V paste into the folder, ⌘D duplicate, ⌘Z
/// and ⇧⌘Z undo and redo. The menu bar's Edit commands send the same actions to the focused view.
pub fn bindings() -> Vec<KeyBinding> {
    let context = Some(CONTEXT);
    vec![
        KeyBinding::new("cmd-c", input::Copy, context),
        KeyBinding::new("cmd-v", input::Paste, context),
        KeyBinding::new("cmd-d", Duplicate, context),
        KeyBinding::new("cmd-z", input::Undo, context),
        KeyBinding::new("cmd-shift-z", input::Redo, context),
    ]
}

/// Handles the core's reply to a drive command.
pub type OnSaid = Rc<dyn Fn(rename::Said, &mut Window, &mut App)>;

/// Lets a drive view's element take the drive's file commands.
pub trait DriveCommands: InteractiveElement + Sized {
    /// Handles Copy, Paste, Duplicate, Undo and Redo on `table` (-1 the list, otherwise a column
    /// number) while `focus` has the keys, passing each reply to `said`. A focused field inside,
    /// such as a rename, keeps these keys for itself.
    fn take_drive_commands(
        self,
        focus: &FocusHandle,
        table: impl Fn() -> i64 + Clone + 'static,
        said: OnSaid,
    ) -> Self {
        let editing = |edit: NrDriveEdit| {
            let (focus, table, said) = (focus.clone(), table.clone(), said.clone());
            move |window: &mut Window, cx: &mut App| {
                if !focus.is_focused(window) {
                    cx.propagate();
                    return;
                }
                said(edit_files(edit, table()), window, cx);
            }
        };
        let (copy, duplicate, undo, redo) = (
            editing(NrDriveEdit::Copy),
            editing(NrDriveEdit::Duplicate),
            editing(NrDriveEdit::Undo),
            editing(NrDriveEdit::Redo),
        );
        let pasting = focus.clone();
        self.on_action(move |_: &input::Copy, window, cx| copy(window, cx))
            .on_action(move |_: &Duplicate, window, cx| duplicate(window, cx))
            .on_action(move |_: &input::Undo, window, cx| undo(window, cx))
            .on_action(move |_: &input::Redo, window, cx| redo(window, cx))
            .on_action(move |_: &input::Paste, window, cx| {
                if !pasting.is_focused(window) {
                    cx.propagate();
                    return;
                }
                if let Some(pasted) = paste_files(table(), cx) {
                    said(pasted, window, cx);
                }
            })
    }
}

impl<E: InteractiveElement> DriveCommands for E {}

/// Copies, duplicates, undoes or redoes in `table` (`nr_drive_edit`).
pub fn edit_files(edit: NrDriveEdit, table: i64) -> rename::Said {
    rename::Said::of(&nr_drive_edit(edit, table))
}

/// Pastes the clipboard's files into `table`'s folder (`nr_drive_paste`). `None` when the
/// clipboard holds no files.
pub fn paste_files(table: i64, cx: &App) -> Option<rename::Said> {
    let files = crate::platform::read_clipboard_files(cx);
    if files.is_empty() {
        return None;
    }
    let lines: Vec<String> = files
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let lines = crate::core::c(&lines.join("\n"));
    // SAFETY: a NUL-terminated list.
    Some(rename::Said::of(&unsafe {
        nr_drive_paste(table, lines.as_ptr())
    }))
}

/// Copies files dropped from outside the app into `column`'s folder (`nr_folder_import`).
pub fn import(column: usize, dropped: &gpui::ExternalPaths) -> rename::Said {
    let paths: Vec<String> = dropped
        .paths()
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    let paths = crate::core::c(&paths.join("\n"));
    // SAFETY: a NUL-terminated list.
    rename::Said::refused(&unsafe { northroom_core::ffi::nr_folder_import(column, paths.as_ptr()) })
}

/// Copies the rows the core lent.
pub fn rows_of(rows: &NrRows) -> Vec<usize> {
    if rows.count == 0 {
        return Vec::new();
    }
    // SAFETY: the core lends `count` rows until its next call.
    unsafe { std::slice::from_raw_parts(rows.rows, rows.count) }.to_vec()
}

/// The selected rows of `table`, sorted and unique, so lookups can binary search.
pub fn selected(table: i64) -> Vec<usize> {
    rows_of(&nr_rows_selected(table))
}

/// Draws a selected row's bar. A run of selected rows reads as one shape, rounded only at its
/// ends, with a hairline between rows inset 16 px. The active table uses the accent surface (a
/// raised grey, never the inverted primary); other tables use the muted surface.
pub fn selection_bar(
    window: &mut Window,
    bar: Bounds<Pixels>,
    worked_in: bool,
    above: bool,
    below: bool,
) {
    let ground = if worked_in {
        color(Token::Accent)
    } else {
        color(Token::Muted)
    };
    let (top, bottom) = (
        if above { 0.0 } else { CONTROL_RADIUS },
        if below { 0.0 } else { CONTROL_RADIUS },
    );
    window.paint_quad(gpui::fill(bar, ground).corner_radii(Corners {
        top_left: px(top),
        top_right: px(top),
        bottom_left: px(bottom),
        bottom_right: px(bottom),
    }));
    if above {
        let line = if worked_in {
            mixed(Token::AccentForeground, 0.10)
        } else {
            mixed(Token::Foreground, 0.10)
        };
        fill(
            window,
            frame(
                f32::from(bar.origin.x) + 16.0,
                f32::from(bar.origin.y),
                f32::from(bar.size.width) - 32.0,
                1.0,
            ),
            line,
        );
    }
}

/// Draws a 2 px ring inside a row to mark the drop target folder. Square unless the row is
/// selected, then it follows the selection's corners.
pub fn drop_ring(window: &mut Window, bar: Bounds<Pixels>, selected: bool) {
    let radius = if selected { CONTROL_RADIUS - 1.0 } else { 0.0 };
    crate::draw::stroke(
        window,
        inset(bar, 1.0, 1.0),
        color(Token::Primary),
        radius,
        2.0,
    );
}

/// Draws a row's icon in its 24 px slot: a folder glyph, a picture or video thumbnail, a 3D
/// model still (`nr_model_still`), a terminal mark, or the system thumbnail for other files.
/// Falls back to the kind's glyph.
pub fn file_icon(
    window: &mut Window,
    cx: &mut App,
    item: &Item,
    slot: Bounds<Pixels>,
    text_color: Hsla,
) {
    let glyph = |window: &mut Window, cx: &mut App| {
        if let Some(glyph) = item.glyph {
            icons::draw(window, cx, glyph, inset(slot, 4.0, 4.0), text_color);
        }
    };
    match item.kind {
        NrFileKind::Folder => folder_icon(window, cx, item, slot, text_color),
        NrFileKind::Terminal => terminal_icon(window, cx, item, slot),
        NrFileKind::Image | NrFileKind::Video => {
            match thumbs::picture(&item.id, 96, item.kind == NrFileKind::Video) {
                Some(image) => crate::draw::cover(window, &image, slot, Corners::all(px(2.0))),
                None => glyph(window, cx),
            }
        }
        NrFileKind::File => {
            if !file_picture(window, item, slot) {
                glyph(window, cx);
            }
        }
        _ => glyph(window, cx),
    }
}

/// Draws a generic file's picture in `slot`: a 3D model's still, or the system thumbnail with
/// a border so a white page shows on a white row. False while loading or when there is none.
fn file_picture(window: &mut Window, item: &Item, slot: Bounds<Pixels>) -> bool {
    let named = crate::core::c(&item.id);
    // SAFETY: a NUL-terminated id.
    let still = text(&unsafe { northroom_core::ffi::nr_model_still(named.as_ptr()) });
    if !still.is_empty() {
        let Some(image) = thumbs::picture_file(&still, 96) else {
            return false;
        };
        crate::draw::contain(window, &image, slot, Corners::default());
        return true;
    }
    let Some(image) = thumbs::system_picture(&item.id, 96) else {
        return false;
    };
    let page = crate::draw::contain(window, &image, slot, Corners::all(px(2.0)));
    crate::draw::stroke(window, page, color(Token::Border), 2.0, 1.0);
    true
}

fn folder_icon(
    window: &mut Window,
    cx: &mut App,
    item: &Item,
    slot: Bounds<Pixels>,
    text_color: Hsla,
) {
    if let Some(glyph) = item.glyph {
        icons::draw(window, cx, glyph, inset(slot, 2.0, 2.0), text_color);
    }
}

/// Draws a terminal's mark (its program's logo or a terminal glyph) on ink, ringed by its
/// activity, with a corner dot while it has something to say.
fn terminal_icon(window: &mut Window, cx: &mut App, item: &Item, slot: Bounds<Pixels>) {
    let look = nr_terminal_look(item.terminal_activity);
    rounded(
        window,
        inset(slot, -1.0, -1.0),
        theme::named(&text(&look.ring)).alpha(look.ring_opacity as f32),
        8.0,
    );
    rounded(window, slot, color(Token::Ink), 7.0);
    let shown = inset(slot, 4.0, 4.0);
    let pixels = f32::from(shown.size.width) * window.scale_factor();
    match thumbs::logo(&item.terminal_logo, pixels, cx) {
        Some(logo) => {
            let _ = window.paint_image(shown, shown, Corners::default(), logo, 0, false);
        }
        None => icons::draw(
            window,
            cx,
            Glyph::TerminalWindow,
            inset(slot, 5.0, 5.0),
            color(Token::Paper),
        ),
    }
    let dot = text(&look.dot);
    if !dot.is_empty() {
        let at = frame(
            f32::from(slot.origin.x + slot.size.width) - 6.0,
            f32::from(slot.origin.y + slot.size.height) - 6.0,
            8.0,
            8.0,
        );
        let sign = text(&look.sign);
        oval(
            window,
            inset(crate::signs::reach(&sign, at), -1.5, -1.5),
            color(Token::Ink),
        );
        crate::signs::paint(
            window,
            cx,
            &sign,
            at,
            theme::faded(theme::named(&dot), look.dot_fade),
        );
    }
}
