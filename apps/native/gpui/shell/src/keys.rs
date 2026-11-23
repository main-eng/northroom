//! Key routing. A key goes first to the focused view (a list, column, board, field, open file,
//! menu, dialog or palette). Keys it ignores reach the window (`window_key`), which runs the menu
//! bar entry with that shortcut, as AppKit key equivalents do (`menu_bar::key`): ⌘K opens the
//! palette, ⌘T a new tab. While a native view has AppKit focus (a live page's web view, a PDF),
//! the window runs nothing: AppKit sends keys to that view, and unhandled ones reach the menu
//! bar, whose items (hidden ones too) carry every shortcut.

use crate::menu_bar::{self, BarEntry, BarMenu};
use crate::workspace::Workspace;
use gpui::{Context, KeyDownEvent, Window};
use kit::core::text;
use kit::nr::*;

/// Finds the entry in `menus` or their submenus with `shortcut`.
fn with_shortcut<'a>(menus: &'a [BarMenu], shortcut: &str) -> Option<&'a BarEntry> {
    menus.iter().find_map(|menu| {
        menu.entries.iter().find_map(|entry| {
            if entry.enabled && entry.shortcut == shortcut {
                return Some(entry);
            }
            entry
                .submenu
                .as_ref()
                .and_then(|submenu| with_shortcut(std::slice::from_ref(submenu), shortcut))
        })
    })
}

/// Handles a key no view took by running the menu bar entry with that shortcut.
pub fn window_key(
    workspace: &mut Workspace,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    #[cfg(target_os = "macos")]
    if kit::platform::hosted_has_keys(window) {
        return;
    }
    let press = kit::keys::press(&event.keystroke);
    if press.held & (NR_HELD_COMMAND | NR_HELD_CONTROL) == 0 {
        return;
    }
    let mut characters = event.keystroke.key.chars();
    let (Some(key), None) = (characters.next(), characters.next()) else {
        return;
    };
    let shortcut = text(&nr_shortcut_text(u32::from(key), press.held));
    let menus = menu_bar::menus();
    let Some(entry) = with_shortcut(&menus, &shortcut) else {
        return;
    };
    if menu_bar::key(entry, workspace, window, cx) {
        cx.stop_propagation();
    }
}
