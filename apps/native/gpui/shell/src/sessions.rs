//! Session alerts outside the window: a system notification when an unseen session starts
//! waiting for the user, and on a Mac the waiting count on the Dock icon. The core decides which
//! sessions are news (`session_news`). Clicking a notification shows its session, like its row in
//! the tray menu (`nr_menu_run`).

use crate::effects;
use crate::workspace::Workspace;
use gpui::{App, SystemNotification, Window, WindowHandle};
use kit::core::c;
use kit::lists::rename::Said;
use kit::nr::*;

/// Makes a clicked notification show its session in `handle`'s window. Call once at launch.
pub fn install(handle: WindowHandle<Workspace>, cx: &mut App) {
    cx.on_system_notification_response(move |response, cx| {
        let _ = handle.update(cx, |workspace, window, cx| {
            let entry = c(&response.tag);
            // SAFETY: a NUL-terminated string.
            let said = Said::of(&unsafe { nr_menu_run(NrMenu::Tray, entry.as_ptr()) });
            effects::carry_out(workspace, said, window, cx);
        });
    });
}

/// Notifies about sessions that just started waiting and updates the Dock count. Checks stay
/// silent.
pub fn tell(window: &Window, cx: &mut App) {
    if nr_launch().unattended {
        return;
    }
    for notice in session_news(window.is_window_active()) {
        cx.show_system_notification(SystemNotification {
            tag: notice.entry.into(),
            title: notice.title.into(),
            body: notice.body.into(),
            actions: Vec::new(),
        });
    }
    #[cfg(target_os = "macos")]
    kit::platform::badge_dock(sessions_waiting());
}
