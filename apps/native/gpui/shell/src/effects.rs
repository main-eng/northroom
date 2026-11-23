//! Carries out the platform side of an `NrEffect` the core returns after a menu entry, dialog
//! answer, rename, drop or palette command.

use crate::workspace::Workspace;
use gpui::{Bounds, Context, Pixels, Window, size};
use kit::lists::rename::Said;
use kit::nr::*;
use std::path::{Path, PathBuf};

pub fn carry_out(
    workspace: &mut Workspace,
    said: Said,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    match said.effect {
        NrEffect::Nothing | NrEffect::Changed | NrEffect::Stays => {}
        NrEffect::Refresh => nr_columns_refresh(),
        NrEffect::Quit => cx.quit(),
        NrEffect::Toast => workspace
            .overlay
            .toast(&said.text, &said.detail, window, cx),
        NrEffect::Prompt => workspace.show_dialog(window, cx),
        NrEffect::TogglePalette => workspace.overlay.toggle_palette(window, cx),
        NrEffect::Rename => workspace.rename(&said.text, window, cx),
        NrEffect::OpenAddress => cx.open_url(&said.text),
        NrEffect::CheckForUpdates => crate::updates::request(true, cx),
        NrEffect::ChooseDrive => choose_drive(window, cx),
        NrEffect::OpenDiagnosticLogs => {
            let folder = kit::core::text(&nr_log_directory());
            if !folder.is_empty() {
                cx.open_with_system(std::path::Path::new(&folder));
            }
        }
        // A check never steals focus from the user's app.
        NrEffect::ShowWindow if nr_launch().unattended => {}
        NrEffect::ShowWindow => {
            cx.activate(true);
            window.activate_window();
        }
        NrEffect::Page => crate::screens::page::menu_chosen(window, cx),
        NrEffect::CopyFiles => kit::platform::write_clipboard_files(&paths_of(&said.text), cx),
        // A folder's menu anchors the sheet where it opened (`overlay::menu`). Here it anchors
        // at the pointer.
        NrEffect::Share => {
            let at = Bounds::new(window.mouse_position(), size(Pixels::ZERO, Pixels::ZERO));
            share(&paths_of(&said.text), at, window);
        }
        NrEffect::Reveal => reveal(&paths_of(&said.text)),
        NrEffect::Paste => {
            let table: i64 = said
                .text
                .parse()
                .expect("the core names the table pasted into");
            if let Some(pasted) = kit::lists::paste_files(table, cx) {
                carry_out(workspace, pasted, window, cx);
            }
        }
        // Only a folder's menu sends these, and it handles them itself (`overlay::menu`).
        NrEffect::Open | NrEffect::ImportFiles => {
            eprintln!(
                "gpui\tthe effect {:?} comes only from a folder's menu",
                said.effect
            )
        }
    }
    workspace.changed(window, cx);
}

/// Lets the user pick a new drive folder (`nr_selection_choose`). A refused folder gets a system
/// alert with the reason.
fn choose_drive(window: &mut Window, cx: &mut Context<Workspace>) {
    let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(kit::core::text(&nr_folder_picker_button()).into()),
    });
    cx.spawn_in(window, async move |workspace, cx| {
        let Ok(Ok(Some(paths))) = picked.await else {
            return;
        };
        let Some(folder) = paths.first() else {
            return;
        };
        let support = kit::core::c(&kit::core::text(&nr_path(NrPath::Support)));
        let folder = kit::core::c(&folder.to_string_lossy());
        // SAFETY: NUL-terminated paths.
        let refusal =
            kit::core::text(&unsafe { nr_selection_choose(support.as_ptr(), folder.as_ptr()) });
        let _ = workspace.update_in(cx, |workspace, window, cx| {
            if !refusal.is_empty() {
                let none: [gpui::PromptButton; 0] = [];
                drop(window.prompt(
                    gpui::PromptLevel::Warning,
                    &kit::core::text(&nr_choose_refused()),
                    Some(&refusal),
                    &none,
                    cx,
                ));
            }
            workspace.changed(window, cx);
        });
    })
    .detach();
}

/// Splits an effect's text into one path per line.
pub fn paths_of(lines: &str) -> Vec<PathBuf> {
    lines.lines().map(PathBuf::from).collect()
}

/// How drive files are handed to the system.
#[derive(Clone, Copy, Debug)]
enum Handed {
    Share,
    Reveal,
}

impl Handed {
    /// The word a check's log writes before each path.
    fn word(self) -> &'static str {
        match self {
            Self::Share => "shared",
            Self::Reveal => "revealed",
        }
    }
}

/// In a check, logs each path as `word<TAB>path` to `NR_OPEN_LOG` (or stderr) instead of opening
/// the share sheet, Finder or another app, matching how the core logs `nr_take_opened_elsewhere`.
/// Returns whether this is a check.
fn logged_in_check(handed: Handed, paths: &[PathBuf]) -> bool {
    use std::io::Write;
    if !nr_launch().unattended {
        return false;
    }
    let lines: String = paths
        .iter()
        .map(|path| format!("{}\t{}\n", handed.word(), path.display()))
        .collect();
    match std::env::var_os("NR_OPEN_LOG") {
        Some(log) => {
            let written = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log)
                .and_then(|mut file| file.write_all(lines.as_bytes()));
            if let Err(error) = written {
                eprintln!("{} was not written: {error}", Path::new(&log).display());
            }
        }
        None => eprint!("{lines}"),
    }
    true
}

/// Opens the system's share sheet for the files at `paths`, pointing at `at` in the window.
pub fn share(paths: &[PathBuf], at: Bounds<Pixels>, window: &Window) {
    if logged_in_check(Handed::Share, paths) {
        return;
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    kit::platform::share(paths, window, at);
    // Linux has no share sheet; the core offers no Share there.
    #[cfg(target_os = "linux")]
    {
        let _ = (at, window);
        eprintln!("gpui\tLinux has no share sheet");
    }
}

/// Shows the files at `paths` selected in the system's file manager.
fn reveal(paths: &[PathBuf]) {
    if !logged_in_check(Handed::Reveal, paths) {
        kit::platform::reveal(paths);
    }
}

/// Carries out effects left by background drive work, such as a failed Compress notice
/// (`nr_drive_work_take`).
pub fn carry_out_left(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    loop {
        let said = Said::of(&nr_drive_work_take());
        if said.effect == NrEffect::Nothing {
            break;
        }
        carry_out(workspace, said, window, cx);
    }
}

/// Shows the system open panel, labelled `button`, and copies the chosen files and folders into
/// column `column`'s folder (`nr_folder_import`).
pub fn import_files(
    column: usize,
    button: &str,
    workspace: gpui::Entity<Workspace>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: true,
        directories: true,
        multiple: true,
        prompt: Some(button.to_owned().into()),
    });
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let _ = handle.update(cx, |_, window, cx| {
            let paths = kit::core::c(&paths.join("\n"));
            // SAFETY: a NUL-terminated list.
            let said = Said::refused(&unsafe { nr_folder_import(column, paths.as_ptr()) });
            workspace.update(cx, |workspace, cx| carry_out(workspace, said, window, cx));
        });
    })
    .detach();
}

/// Opens a file the window can't show in its default app (`nr_take_opened_elsewhere`). Checks
/// never reach this; the core logs to `NR_OPEN_LOG` instead.
pub fn open_elsewhere(path: &str, cx: &gpui::App) {
    cx.open_with_system(std::path::Path::new(path));
}
