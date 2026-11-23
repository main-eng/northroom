//! The rename field over a row, built on the controls' field. It has no background or border;
//! the name sits in the row's font 5 pt in, fully selected, in the selected row's colour with
//! the accent behind selected text. The core places it (the `rename` rect of
//! `nr_list_row_layout` and `nr_column_row_layout`).
//!
//! Return, or losing focus, hands the name to the core (`nr_folder_rename` in the list,
//! `nr_column_rename` in a column), which trims it and ignores an unchanged or blank name
//! (`renamed_title`). Escape cancels.

use crate::controls::input::{Command, Committed, Field, Ground, Keys};
use crate::core::{c, frame, text};
use crate::theme::{Face, Token};
use gpui::{AppContext, Bounds, Context, Entity, Pixels, Subscription, Window};
use northroom_core::ffi::{NrText, nr_column_rename, nr_folder_rename};
use northroom_core::menus::ffi::{NrEffect, NrMenuEffect, nr_refused};
use std::cell::Cell;
use std::rc::Rc;

/// Text inset from the field's edge (AppKit's cell inset plus text container padding), on a
/// 16 px line centred in the field.
const WORDS_IN: f32 = 5.0;
const LINE: f32 = 16.0;

/// A copy of the core's reply (`NrMenuEffect`): the effect and its message.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub effect: NrEffect,
    pub text: String,
    pub detail: String,
}

impl Said {
    pub fn of(effect: &NrMenuEffect) -> Self {
        Self {
            effect: effect.effect,
            text: text(&effect.text),
            detail: text(&effect.detail),
        }
    }

    /// A file operation's result: changed, or the refusal reason.
    pub fn refused(refusal: &NrText) -> Self {
        let refusal = c(&text(refusal));
        // SAFETY: a NUL-terminated refusal.
        Self::of(&unsafe { nr_refused(refusal.as_ptr()) })
    }
}

/// Which file is being renamed, as the FFI identifies it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Renamed {
    /// The file with `id` in `column`'s folder (`nr_folder_rename`), used by the list.
    File { column: usize, id: String },
    /// Row `row` of column `column` (`nr_column_rename`).
    Row { column: usize, row: usize },
}

impl Renamed {
    /// Sends the typed name to the core.
    fn commit(&self, typed: &str) -> Said {
        let typed = c(typed);
        let refusal = match self {
            Renamed::File { column, id } => {
                let id = c(id);
                // SAFETY: NUL-terminated strings.
                unsafe { nr_folder_rename(*column, id.as_ptr(), typed.as_ptr()) }
            }
            // SAFETY: a NUL-terminated title.
            Renamed::Row { column, row } => unsafe {
                nr_column_rename(*column, *row, typed.as_ptr())
            },
        };
        Said::refused(&refusal)
    }
}

/// How a rename ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Ended {
    /// The typed name went to the core.
    Committed(Said),
    /// Escape: nothing changed.
    Cancelled,
}

/// An active rename: the file's id and the field.
pub struct Rename {
    pub id: String,
    pub field: Entity<Field>,
    _events: [Subscription; 2],
}

impl Rename {
    /// Starts renaming `title`, the name of file `id`, sent to the core as `renamed` says.
    /// `ended` runs once, when the rename commits or is cancelled.
    pub fn begin<V: 'static>(
        id: &str,
        renamed: Renamed,
        title: &str,
        window: &mut Window,
        cx: &mut Context<V>,
        ended: impl Fn(&mut V, Ended, &mut Window, &mut Context<V>) + 'static,
    ) -> Self {
        let field = cx.new(|cx| {
            let mut field = Field::new(title, title, window, cx);
            field.ground = Ground::Bare;
            field.face = Face::sans(12.0, 400.0);
            field.ink = Token::AccentForeground;
            field.keys = Keys::Dialog;
            field.editing.select_all();
            field
        });
        let (ended, renamed, done) = (Rc::new(ended), Rc::new(renamed), Rc::new(Cell::new(false)));
        let on_command = {
            let (ended, renamed, done) = (ended.clone(), renamed.clone(), done.clone());
            cx.subscribe_in(
                &field,
                window,
                move |view, field, command: &Command, window, cx| {
                    let end = match command {
                        Command::Return => {
                            Ended::Committed(renamed.commit(&field.read(cx).editing.content))
                        }
                        Command::Escape => Ended::Cancelled,
                        Command::Tab | Command::BackTab => return,
                    };
                    if !done.replace(true) {
                        ended(view, end, window, cx);
                    }
                },
            )
        };
        // Losing focus commits the typed name, as AppKit's field does.
        let on_leave = cx.subscribe_in(
            &field,
            window,
            move |view, _, typed: &Committed, window, cx| {
                if !done.replace(true) {
                    ended(view, Ended::Committed(renamed.commit(&typed.0)), window, cx);
                }
            },
        );
        field.update(cx, |field, cx| field.focus(window, cx));
        Self {
            id: id.to_owned(),
            field,
            _events: [on_command, on_leave],
        }
    }

    /// Places the field at `at` (window points), the core's rect over the row.
    pub fn place<V>(&self, at: Bounds<Pixels>, cx: &mut Context<V>) {
        let words = frame(
            f32::from(at.origin.x) + WORDS_IN,
            f32::from(at.origin.y) + (f32::from(at.size.height) - LINE) / 2.0,
            f32::from(at.size.width) - 2.0 * WORDS_IN,
            LINE,
        );
        self.field.update(cx, |field, cx| {
            if field.frame != at || field.words != words {
                field.place(at, words, cx);
            }
        });
    }
}
