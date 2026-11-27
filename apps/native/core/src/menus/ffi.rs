//! The FFI for menus, dialogs and their keys (ADR-0001): every menu as JSON (`menus::Menu`),
//! what choosing an entry does (`nr_menu_run` does the core's part and returns the shell's),
//! the dialogs a menu opens (`nr_prompt`), keys in drawn menus (`menus::keys`) and dialogs
//! (`dialog::keys`), and list header presses (`list::header`). Returned text is borrowed UTF-8,
//! valid until the next call here that returns the same kind of text.

use crate::columns::{Columns, Table, View};
use crate::dialog::keys::{Dialog, Focus};
use crate::ffi::{text_of, watch_open_columns, with_columns, NrKey, NrKeyPress, NrRect, NrText};
use crate::menus::{self, BarCommand, FolderCommand, ListCommand, Prompt};
use crate::workspace::{DriveError, DriveResult, Entry, EntryKind};
use crate::Relock;
use std::collections::VecDeque;
use std::ffi::c_char;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

fn lend(text: String) -> NrText {
    static LENT: Mutex<String> = Mutex::new(String::new());
    NrText::lend_into(&LENT, text)
}

/// The interface language for every menu and dialog here.
fn language() -> &'static str {
    crate::ffi::interface_language()
}

// MARK: Effects

/// What the shell must do after choosing an entry, answering a dialog or dropping files, once
/// the core has done its part.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Effect {
    Nothing,
    /// Redraw the view from core state.
    Changed,
    /// Reread the open folders from disk.
    Refresh,
    CheckForUpdates,
    /// Open the system folder picker to choose another drive (`nr_selection_choose`).
    ChooseDrive,
    OpenDiagnosticLogs,
    OpenAddress(&'static str),
    TogglePalette,
    ShowWindow,
    /// Redraw and show a notice explaining what went wrong.
    Toast(&'static str, String),
    /// Open the file the menu was opened on.
    Open,
    /// Rename the file with this id, inline where the view supports it, else via
    /// `nr_prompt_rename`.
    Rename(String),
    /// A dialog is ready (`nr_prompt`).
    Prompt,
    /// Open the platform's open panel to pick files to copy into the folder, with this button
    /// label.
    Import(&'static str),
    /// Keep the dialog open, since a name is required.
    Stays,
    /// The open page handled the choice; the shell's part is in `ffi::page_menu_answer`.
    Page,
    /// Copy these files to the clipboard, as files and as path text.
    CopyFiles(Vec<PathBuf>),
    /// Open the share sheet for these files.
    Share(Vec<PathBuf>),
    /// Show these files in the system file manager.
    Reveal(Vec<PathBuf>),
    /// Copy the clipboard's files into this table's folder (`nr_drive_paste`).
    Paste(i64),
    /// The app quits.
    Quit,
}

/// The shell's remaining work after choosing an entry, answering a dialog or dropping files
/// (`NrMenuEffect::effect`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrEffect {
    Nothing,
    /// Redraw.
    Changed,
    /// Reread the folders.
    Refresh,
    CheckForUpdates,
    /// Ask the system folder picker for another drive and pass it to `nr_selection_choose`.
    ChooseDrive,
    /// Open the diagnostic logs' folder.
    OpenDiagnosticLogs,
    /// Open the URL `text`.
    OpenAddress,
    /// Toggle the command palette.
    TogglePalette,
    ShowWindow,
    /// Redraw and show the notice `text` with `detail` under it.
    Toast,
    /// Open the file the menu was opened on.
    Open,
    /// Rename the file `text`.
    Rename,
    /// Show the dialog `nr_prompt` describes.
    Prompt,
    /// Show the open panel with button label `text`.
    ImportFiles,
    /// The dialog stays open.
    Stays,
    /// The open page handled the choice (`ffi::page_menu_answer`).
    Page,
    /// Copy the files `text` (newline-joined absolute paths) to the clipboard, as files and as
    /// path text.
    CopyFiles,
    /// Open the share sheet for the files `text`, anchored where the menu was opened.
    Share,
    /// Show the files `text` in the system file manager.
    Reveal,
    /// Read the clipboard's files and copy them in with `nr_drive_paste`, for the table `text`
    /// names (-1 for the list, n for column n).
    Paste,
    /// The app quits.
    Quit,
}

/// An effect for the FFI: the remaining action (`NrEffect`) and its text.
#[repr(C)]
pub struct NrMenuEffect {
    pub effect: NrEffect,
    pub text: NrText,
    pub detail: NrText,
}

fn hand_over(effect: Effect) -> NrMenuEffect {
    static WORDS: Mutex<(String, String)> = Mutex::new((String::new(), String::new()));
    let (number, text, detail) = match effect {
        Effect::Nothing => (NrEffect::Nothing, String::new(), String::new()),
        Effect::Changed => (NrEffect::Changed, String::new(), String::new()),
        Effect::Refresh => (NrEffect::Refresh, String::new(), String::new()),
        Effect::CheckForUpdates => (NrEffect::CheckForUpdates, String::new(), String::new()),
        Effect::ChooseDrive => (NrEffect::ChooseDrive, String::new(), String::new()),
        Effect::OpenDiagnosticLogs => (NrEffect::OpenDiagnosticLogs, String::new(), String::new()),
        Effect::OpenAddress(address) => (NrEffect::OpenAddress, address.to_owned(), String::new()),
        Effect::TogglePalette => (NrEffect::TogglePalette, String::new(), String::new()),
        Effect::ShowWindow => (NrEffect::ShowWindow, String::new(), String::new()),
        Effect::Toast(title, detail) => (NrEffect::Toast, title.to_owned(), detail),
        Effect::Open => (NrEffect::Open, String::new(), String::new()),
        Effect::Rename(id) => (NrEffect::Rename, id, String::new()),
        Effect::Prompt => (NrEffect::Prompt, String::new(), String::new()),
        Effect::Import(button) => (NrEffect::ImportFiles, button.to_owned(), String::new()),
        Effect::Stays => (NrEffect::Stays, String::new(), String::new()),
        Effect::Page => (NrEffect::Page, String::new(), String::new()),
        Effect::CopyFiles(paths) => (NrEffect::CopyFiles, joined(&paths), String::new()),
        Effect::Share(paths) => (NrEffect::Share, joined(&paths), String::new()),
        Effect::Reveal(paths) => (NrEffect::Reveal, joined(&paths), String::new()),
        Effect::Paste(table) => (NrEffect::Paste, table.to_string(), String::new()),
        Effect::Quit => (NrEffect::Quit, String::new(), String::new()),
    };
    let mut words = WORDS.relock();
    *words = (text, detail);
    NrMenuEffect {
        effect: number,
        text: NrText::borrowing(&words.0),
        detail: NrText::borrowing(&words.1),
    }
}

/// Paths as the shell receives them: absolute, one per line.
fn joined(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Starts an inline rename of a newly created page, folder or terminal (by id), as Finder's
/// New Folder does (`Effect::Rename`), or reports a refusal.
fn made(outcome: Option<DriveResult<Option<String>>>) -> Effect {
    watch_open_columns();
    match outcome {
        Some(Err(error)) => Effect::Toast(menus::update_failed(language()), error.say(language())),
        Some(Ok(Some(id))) => Effect::Rename(id),
        Some(Ok(None)) | None => Effect::Changed,
    }
}

/// The effect of a drive change: a redraw, or a notice of why the drive refused it.
fn changed(outcome: Option<DriveResult<()>>) -> Effect {
    watch_open_columns();
    match outcome {
        Some(Err(error)) => Effect::Toast(menus::update_failed(language()), error.say(language())),
        _ => Effect::Changed,
    }
}

// MARK: The menu bar and the tray

/// A core menu, as the shell names it when requesting it and when an entry is chosen
/// (`nr_menu_run`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMenu {
    /// The menu bar.
    Bar,
    Tray,
    /// A board card's menu (`menu_folder`).
    Card,
    /// A list or column row's menu (`menu_folder`).
    File,
    /// The folder background menu (`menu_folder`).
    Background,
    /// The list's options (`menu_list_options`).
    ListOptions,
    /// A page block's menu (`menu_page_block`).
    PageBlock,
    /// A table cell's menu (`menu_page_table`).
    PageTable,
    /// A tab's menu (`menu_tab`).
    Tab,
    /// The title bar sessions button's menu (`menu_sessions`).
    Sessions,
}

fn bar_state<'a>(
    update_label: &'a str,
    phones: &'a [(crate::remote::devices::Key, String)],
) -> menus::BarState<'a> {
    menus::BarState {
        phones,
        update_label,
        update_enabled: crate::update::ffi::menu_enabled(),
        folder_shows: with_columns(|_| ()).is_some() && crate::ffi::nr_settings_tab() < 0,
        drive_changeable: crate::paths::get().fixed_drive.is_none(),
        view: crate::ffi::folder_view(),
        language: language(),
    }
}

fn update_label() -> String {
    crate::update::ffi::menu_label().to_owned()
}

/// The menu bar's menus.
pub fn menu_bar() -> Vec<menus::Menu> {
    let label = update_label();
    let phones = crate::remote::app::devices();
    menus::menu_bar(bar_state(&label, &phones))
}

/// The tray's menu.
pub fn menu_tray() -> menus::Menu {
    menus::tray(
        &update_label(),
        crate::update::ffi::menu_enabled(),
        &crate::sessions::ffi::running_now(),
        language(),
    )
}

/// The tooltip on the menu bar's Northroom icon.
#[no_mangle]
pub extern "C" fn nr_menu_tray_tooltip() -> NrText {
    NrText::borrowing(menus::TRAY_TOOLTIP)
}

fn run_bar(command: BarCommand) -> Effect {
    use crate::ffi::{
        nr_history_back, nr_history_forward, nr_settings_open, nr_tab_closable, nr_tab_close,
        nr_tab_open, nr_tab_reopen_closed, nr_tabs_active, set_folder_view, toggle_hidden_files,
    };
    match command {
        BarCommand::CheckForUpdates => Effect::CheckForUpdates,
        BarCommand::ShowSettings => {
            nr_settings_open();
            Effect::Changed
        }
        BarCommand::ChooseDrive => Effect::ChooseDrive,
        BarCommand::ConnectPhone => connect_phone(),
        BarCommand::ForgetPhone(key) => ask_to_forget_phone(key),
        BarCommand::ShowView(view) => {
            set_folder_view(view);
            Effect::Changed
        }
        BarCommand::Reload => Effect::Refresh,
        BarCommand::ToggleHiddenFiles => {
            toggle_hidden_files();
            Effect::Changed
        }
        BarCommand::TogglePalette => Effect::TogglePalette,
        BarCommand::NewTab => {
            nr_tab_open();
            Effect::Changed
        }
        // The last tab closes the window instead.
        BarCommand::CloseTab if nr_tab_closable(nr_tabs_active()) => {
            nr_tab_close(nr_tabs_active());
            Effect::Changed
        }
        // The last tab closes the window, which quits.
        BarCommand::CloseTab | BarCommand::Quit => quit_or_ask(),
        BarCommand::ReopenClosedTab => {
            nr_tab_reopen_closed();
            Effect::Changed
        }
        BarCommand::Back => {
            nr_history_back();
            Effect::Changed
        }
        BarCommand::Forward => {
            nr_history_forward();
            Effect::Changed
        }
        BarCommand::OpenDiagnosticLogs => Effect::OpenDiagnosticLogs,
        BarCommand::OpenHelp => Effect::OpenAddress(menus::HELP_ADDRESS),
        BarCommand::ShowWindow => Effect::ShowWindow,
        BarCommand::ShowNextSession => show_next_session(),
    }
}

// MARK: The sessions' menu

/// The title bar sessions button's menu for the current sessions (`menus::sessions_menu`).
pub fn menu_sessions() -> menus::Menu {
    menus::sessions_menu(&crate::sessions::ffi::running_now(), language())
}

/// Shows terminal file `id`'s session in its own tab (`Columns::show_in_tab`).
fn show_session(id: &str) -> Effect {
    changed(with_columns(|columns| columns.show_in_tab(id)))
}

/// Shows a session chosen from outside the window (the menu bar icon, a notification). The
/// window is told to redraw, since nothing in it was pressed, and comes to the front.
fn show_session_in_front(id: &str) -> Effect {
    match show_session(id) {
        Effect::Changed => {
            crate::notify_changed();
            Effect::ShowWindow
        }
        refused => refused,
    }
}

/// Goes to the next session waiting for the person (`sessions::next`), starting after the one
/// in the active tab; does nothing when none is waiting.
fn show_next_session() -> Effect {
    let shown = with_columns(|columns| {
        columns
            .tab_terminal(columns.active_tab())
            .map(|opened| opened.id.clone())
    })
    .flatten();
    let running = crate::sessions::ffi::running_now();
    match crate::sessions::next(&running, shown.as_deref()) {
        Some(session) => show_session(&session.id),
        None => Effect::Nothing,
    }
}

/// Runs a command palette command (`nr_palette_key`'s `run`), with the same effect as its menu
/// item.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_palette_run(id: *const c_char) -> NrMenuEffect {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    hand_over(menus::palette_command(&id).map_or(Effect::Nothing, run_bar))
}

// MARK: A folder's menus

/// What the currently open folder menu targets.
struct FolderMenu {
    which: NrMenu,
    column: usize,
    clicked: String,
    group: Vec<String>,
}

static FOLDER_MENU: Mutex<Option<FolderMenu>> = Mutex::new(None);

fn menu_file(entry: Entry) -> menus::File {
    let beside = beside(&entry);
    menus::File {
        beside,
        terminal: entry.kind == EntryKind::Terminal,
        running: entry.kind == EntryKind::Terminal
            && crate::ffi::session_activity(&entry.id).is_some(),
        protection: entry
            .protection
            .map(|protection| crate::strings::text(language(), protection.words())),
        id: entry.id,
        title: entry.title,
    }
}

/// Whether `entry`'s folder accepts a new file beside it, as Compress writes there
/// (`Columns::plan_compress`).
fn beside(entry: &Entry) -> menus::Beside {
    let planned =
        with_columns(|columns| columns.plan_compress(std::slice::from_ref(&entry.id), language()));
    match planned {
        Some(Ok(Some(_))) => menus::Beside::TakesFiles,
        Some(Ok(None) | Err(_)) | None => menus::Beside::Protected,
    }
}

/// The current paths of files `ids`, skipping any that are gone.
fn paths_of(ids: &[String]) -> Vec<PathBuf> {
    with_columns(|columns| {
        ids.iter()
            .filter_map(|id| columns.workspace().path_of(id).ok())
            .collect()
    })
    .unwrap_or_default()
}

/// The `effect` for files `ids`; nothing when none exists.
fn with_paths(ids: &[String], effect: fn(Vec<PathBuf>) -> Effect) -> Effect {
    let paths = paths_of(ids);
    if paths.is_empty() {
        return Effect::Nothing;
    }
    effect(paths)
}

/// Whether the clipboard holds files, as last reported by the shell
/// (`nr_set_clipboard_holds_files`).
static CLIPBOARD_HOLDS_FILES: AtomicBool = AtomicBool::new(false);

/// Sets whether the clipboard holds files. The shell calls this before requesting a folder
/// background menu, whose Paste is enabled only then.
#[no_mangle]
pub extern "C" fn nr_set_clipboard_holds_files(holds: bool) {
    CLIPBOARD_HOLDS_FILES.store(holds, Ordering::Relaxed);
}

fn clipboard() -> menus::Clipboard {
    if CLIPBOARD_HOLDS_FILES.load(Ordering::Relaxed) {
        menus::Clipboard::Files
    } else {
        menus::Clipboard::Other
    }
}

/// The files `ids` as a column's folder lists them, or as the list shows them inside an
/// expanded folder, skipping any that are gone.
fn entries(column: usize, ids: &[String]) -> Vec<Entry> {
    let listed = with_columns(|columns| columns.entries_of(column, ids))
        .and_then(Result::ok)
        .unwrap_or_default();
    let rows = crate::app().list_rows.relock();
    ids.iter()
        .filter_map(|id| {
            listed.iter().find(|entry| &entry.id == id).or_else(|| {
                rows.iter()
                    .map(|row| &row.entry)
                    .find(|entry| &entry.id == id)
            })
        })
        .cloned()
        .collect()
}

/// A folder menu, remembering its target for `nr_menu_run`. `which` is a board card's
/// (`clicked`), a list or column row's (`clicked`, plus `selected`, the rows selected with it),
/// or the folder background's. `column` is the column whose folder the menu acts in.
pub fn menu_folder(
    which: NrMenu,
    column: usize,
    clicked: &str,
    selected: &[String],
) -> menus::Menu {
    let view = crate::ffi::folder_view();
    let group: Vec<String> = match which {
        NrMenu::File => menus::menu_group(clicked, selected)
            .into_iter()
            .map(str::to_owned)
            .collect(),
        NrMenu::Card => vec![clicked.to_owned()],
        _ => Vec::new(),
    };
    let listed = entries(column, &group);
    let files: Vec<menus::File> = listed.into_iter().map(menu_file).collect();
    let desktop = menus::Desktop::this();
    let menu = match which {
        NrMenu::Card | NrMenu::File if files.is_empty() => menus::Menu::new("", Vec::new()),
        NrMenu::Card | NrMenu::File => menus::file_menu(&files, view, desktop, language()),
        NrMenu::Background => menus::background_menu(
            crate::ffi::folder_refusal(column),
            clipboard(),
            view,
            with_columns(|columns| columns.list_sort().map(|sort| sort.key)).flatten(),
            language(),
        ),
        _ => menus::Menu::new("", Vec::new()),
    };
    *FOLDER_MENU.relock() = Some(FolderMenu {
        which,
        column,
        clicked: clicked.to_owned(),
        group,
    });
    menu
}

fn run_folder(menu: &FolderMenu, command: FolderCommand) -> Effect {
    let (column, group) = (menu.column, &menu.group);
    match command {
        FolderCommand::Open => Effect::Open,
        FolderCommand::EndSession => end_session_or_ask(&menu.clicked),
        FolderCommand::OpenInNewTab => {
            changed(with_columns(|columns| columns.show_in_tab(&menu.clicked)))
        }
        FolderCommand::Rename => Effect::Rename(menu.clicked.clone()),
        FolderCommand::Duplicate => changed(with_columns(|columns| columns.duplicate(group))),
        FolderCommand::Compress => start_compress(group),
        FolderCommand::Copy => with_paths(group, Effect::CopyFiles),
        FolderCommand::Share => with_paths(group, Effect::Share),
        FolderCommand::Reveal => with_paths(group, Effect::Reveal),
        FolderCommand::Trash => changed(with_columns(|columns| columns.trash(group))),
        FolderCommand::Paste => Effect::Paste(table_number(column)),
        FolderCommand::SortBy(number) => {
            if let Some(key) = crate::list::SortKey::numbered(number) {
                with_columns(|columns| columns.sort_by(key));
            }
            Effect::Changed
        }
        FolderCommand::ShowView(view) => {
            crate::ffi::set_folder_view(view);
            Effect::Changed
        }
        FolderCommand::NewPage => made(with_columns(|columns| {
            columns.create_page(column, language())
        })),
        FolderCommand::NewFolder => made(with_columns(|columns| {
            columns.create_folder(column, language())
        })),
        FolderCommand::NewTerminal => made(with_columns(|columns| {
            columns.create_terminal(column, language())
        })),
        FolderCommand::Import => Effect::Import(menus::import_button(language())),
    }
}

// MARK: The list header's options

static LIST_MOVING: Mutex<Option<u32>> = Mutex::new(None);

/// The keys of the shown columns after Name.
fn shown_columns() -> Vec<u32> {
    with_columns(|columns| {
        columns
            .list_columns()
            .iter()
            .skip(1)
            .map(|column| column.number())
            .collect()
    })
    .unwrap_or_default()
}

/// The list options, after the Move items for column key `moving` (`None` when opened from the
/// options button).
pub fn menu_list_options(moving: Option<u32>) -> menus::Menu {
    let shown = shown_columns();
    let named = moving.map(|key| menus::Moving {
        key,
        at: shown.iter().position(|shown| *shown == key).unwrap_or(0),
        count: shown.len(),
    });
    let menu = with_columns(|columns| {
        let choices: Vec<menus::Choice> = crate::list::Column::ALL
            .iter()
            .map(|column| {
                (
                    column.number(),
                    column.label(),
                    columns.list_columns().contains(column),
                )
            })
            .collect();
        menus::list_options(named, &choices, columns.folders_first(), language())
    })
    .unwrap_or_else(|| menus::Menu::new("", Vec::new()));
    *LIST_MOVING.relock() = moving;
    menu
}

fn run_list(command: ListCommand) -> Effect {
    match command {
        ListCommand::MoveColumn(to) => {
            if let Some(key) = *LIST_MOVING.relock() {
                crate::ffi::list_move_column(key, to);
            }
        }
        ListCommand::ToggleColumn(key) => crate::ffi::list_toggle_column(key),
        ListCommand::ToggleFoldersFirst => {
            with_columns(|columns| columns.toggle_folders_first());
        }
        ListCommand::Restore => {
            with_columns(|columns| columns.reset_list_options());
        }
    }
    Effect::Changed
}

/// Runs entry `id` of menu `which`. The core does its part; the effect says what the shell
/// must do.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_menu_run(which: NrMenu, id: *const c_char) -> NrMenuEffect {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    let effect = match which {
        NrMenu::Bar => menus::bar_command(&id).map_or(Effect::Nothing, run_bar),
        // Tray entries are also what a session notification runs when clicked.
        NrMenu::Tray => match menus::session_entry(&id) {
            Some(file) => show_session_in_front(file),
            None => menus::bar_command(&id).map_or(Effect::Nothing, run_bar),
        },
        NrMenu::ListOptions => menus::list_command(&id).map_or(Effect::Nothing, run_list),
        NrMenu::Sessions => match (menus::session_entry(&id), menus::end_session_entry(&id)) {
            (Some(file), _) => show_session(file),
            (_, Some(file)) => end_session_or_ask(file),
            _ => menus::bar_command(&id).map_or(Effect::Nothing, run_bar),
        },
        NrMenu::PageBlock => menus::page_block_command(&id).map_or(Effect::Nothing, |command| {
            crate::ffi::run_page_block_menu(command);
            Effect::Page
        }),
        NrMenu::PageTable => menus::page_table_command(&id).map_or(Effect::Nothing, |command| {
            crate::ffi::run_page_table_menu(command);
            Effect::Page
        }),
        NrMenu::Tab => match (id.as_str(), *TAB_MENU.relock()) {
            (menus::CLOSE_TAB_ENTRY, Some(tab)) if crate::ffi::nr_tab_closable(tab) => {
                crate::ffi::nr_tab_close(tab);
                Effect::Changed
            }
            (menus::REOPEN_CLOSED_TAB_ENTRY, _) => {
                crate::ffi::nr_tab_reopen_closed();
                Effect::Changed
            }
            _ => Effect::Nothing,
        },
        NrMenu::Card | NrMenu::File | NrMenu::Background => {
            let menu = FOLDER_MENU.relock().take();
            let effect = match (&menu, menus::folder_command(&id)) {
                (Some(menu), Some(command)) if menu.which == which => run_folder(menu, command),
                _ => Effect::Nothing,
            };
            // A menu that stays open can be chosen from again.
            *FOLDER_MENU.relock() = menu;
            effect
        }
    };
    hand_over(effect)
}

// MARK: A tab's menu

/// The tab whose menu is open (`menu_tab`).
static TAB_MENU: Mutex<Option<usize>> = Mutex::new(None);

/// The context menu for a right click at (`x`, `y`) on the title bar (`menus::tab_menu`), for
/// the tab under the point; its entries run as menu 9. Empty when no tab is there.
pub fn menu_tab(x: f64, y: f64) -> menus::Menu {
    let tab = crate::window::ffi::title_bar_tab_at(x, y);
    *TAB_MENU.relock() = tab;
    tab.map_or_else(
        || menus::Menu::new("", Vec::new()),
        |tab| {
            menus::tab_menu(
                crate::ffi::nr_tab_closable(tab),
                crate::ffi::nr_tab_reopenable(),
                language(),
            )
        },
    )
}

// MARK: The page's menus

/// The menu for block `block` of the open page, opened from its handle
/// (`menus::page_block_menu`); its entries run as menu 7.
pub fn menu_page_block(block: usize) -> menus::Menu {
    crate::ffi::page_block_kind(block).map_or_else(
        || menus::Menu::new("", Vec::new()),
        |kind| menus::page_block_menu(&kind, language()),
    )
}

/// The context menu for a cell in `row` of table `block` on the open page
/// (`menus::page_table_menu`); its entries run as menu 8.
pub fn menu_page_table(block: usize, row: usize) -> menus::Menu {
    menus::page_table_menu(row, crate::ffi::page_table_columns(block), language())
}

/// Formats a shell-built entry's shortcut as the core's menus do (`menus::Shortcut::text`):
/// `key` is its character, `held` its modifiers (`menus::COMMAND`…).
#[no_mangle]
pub extern "C" fn nr_shortcut_text(key: u32, held: u32) -> NrText {
    lend(
        char::from_u32(key).map_or_else(String::new, |key| menus::Shortcut::with(key, held).text()),
    )
}

// MARK: Dialogs

/// The action a dialog's confirmation performs.
#[derive(Clone, Debug)]
enum Ask {
    Rename(usize, String),
    /// Open this locked file with the password typed.
    Unlock(std::path::PathBuf),
    /// End this terminal file's session once confirmed.
    EndSession(String),
    /// Quit once confirmed.
    Quit,
    /// A phone's pairing code is on screen; either answer takes it off.
    Pair,
    /// Forget the paired phone with this key once confirmed.
    ForgetPhone(crate::remote::devices::Key),
}

static PROMPT: Mutex<Option<(Ask, Prompt)>> = Mutex::new(None);

/// The locked file last asked about (`nr_prompt_unlock`).
static UNLOCK_ASKED: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

/// Lets a locked file ask for its password again, as when it is opened anew.
#[no_mangle]
pub extern "C" fn nr_unlock_forget_asked() {
    *UNLOCK_ASKED.relock() = None;
}

fn ask(ask: Ask, prompt: Prompt) -> Effect {
    *PROMPT.relock() = Some((ask, prompt));
    Effect::Prompt
}

/// Opens a rename dialog for file `id` in a column's folder, for views without inline rename.
/// False when the file is not there.
///
/// # Safety
/// `id` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_prompt_rename(column: usize, id: *const c_char) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let id = unsafe { text_of(id) };
    let Some(file) = entries(column, std::slice::from_ref(&id))
        .into_iter()
        .next()
    else {
        return false;
    };
    ask(
        Ask::Rename(column, id),
        menus::rename_prompt(&file.title, language()),
    ) == Effect::Prompt
}

/// A dialog whose keys the core handles (`dialog::keys::Dialog`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrDialog {
    /// A dialog that asks for a name.
    Prompt,
    Alert,
}

impl NrDialog {
    fn of(dialog: Dialog) -> Self {
        match dialog {
            Dialog::Prompt => Self::Prompt,
            Dialog::Alert => Self::Alert,
        }
    }

    fn dialog(self) -> Dialog {
        match self {
            Self::Prompt => Dialog::Prompt,
            Self::Alert => Dialog::Alert,
        }
    }
}

/// The dialog to show (`menus::Prompt`): its title, the sentence under it when `has_message`,
/// the field and its initial value when `has_field`, and its button labels. `dialog` selects
/// its key handling (`nr_dialog_key`): a name prompt or an alert.
#[repr(C)]
pub struct NrPrompt {
    pub dialog: NrDialog,
    pub title: NrText,
    pub message: NrText,
    pub has_message: bool,
    pub value: NrText,
    pub has_field: bool,
    pub action: NrText,
    pub cancel: NrText,
    pub destructive: bool,
    /// The field holds a password, shown as bullets.
    pub secret: bool,
}

/// The most recently requested dialog. Its text is valid until another is requested or it is
/// answered.
#[no_mangle]
pub extern "C" fn nr_prompt() -> NrPrompt {
    let prompt = PROMPT.relock();
    let empty = NrText::borrowing("");
    let Some((_, prompt)) = prompt.as_ref() else {
        return NrPrompt {
            dialog: NrDialog::Alert,
            title: NrText::borrowing(""),
            message: NrText::borrowing(""),
            has_message: false,
            value: NrText::borrowing(""),
            has_field: false,
            action: NrText::borrowing(""),
            cancel: NrText::borrowing(""),
            destructive: false,
            secret: false,
        };
    };
    NrPrompt {
        dialog: NrDialog::of(if prompt.value.is_some() {
            Dialog::Prompt
        } else {
            Dialog::Alert
        }),
        title: NrText::borrowing(&prompt.title),
        message: prompt
            .message
            .as_deref()
            .map_or(NrText::borrowing(""), NrText::borrowing),
        has_message: prompt.message.is_some(),
        value: prompt.value.as_deref().map_or(empty, NrText::borrowing),
        has_field: prompt.value.is_some(),
        action: NrText::borrowing(prompt.buttons.map_or("", |buttons| buttons.action)),
        cancel: NrText::borrowing(prompt.buttons.map_or("", |buttons| buttons.cancel)),
        destructive: prompt.destructive,
        secret: prompt.secret,
    }
}

/// Layout of the current dialog over a `width` by `height` view (`dialog::layout::PromptLayout`):
/// the panel, a prompt's close button, the title and message lines, the field background and
/// where the platform field sits on it, and the Cancel and action buttons with their labels,
/// when it has them.
pub fn prompt_layout(width: f64, height: f64) -> crate::dialog::layout::PromptLayout {
    let prompt = PROMPT
        .relock()
        .as_ref()
        .map(|(_, prompt)| prompt.clone())
        .unwrap_or(Prompt {
            title: String::new(),
            message: None,
            value: None,
            buttons: None,
            destructive: false,
            secret: false,
            code: None,
        });
    let color = crate::dialog::keys::action_text_color(
        prompt.destructive,
        crate::ffi::nr_appearance_is_dark(),
    );
    crate::dialog::layout::prompt(
        &prompt,
        width,
        height,
        color,
        &mut crate::text::typeset::width,
    )
}

/// Layout of a shell-drawn alert over a `width` by `height` view (`dialog::layout::AlertLayout`),
/// for platforms whose native alert cannot bind Return to a chosen button: `message`, `detail`
/// and `buttons` in order, with Return on `return_button`, shown in the warning colour for a
/// `critical` alert. With no buttons it gets the platform's OK.
pub fn alert_layout(
    width: f64,
    height: f64,
    message: &str,
    detail: &str,
    buttons: &[&str],
    return_button: usize,
    critical: bool,
) -> crate::dialog::layout::AlertLayout {
    let color =
        crate::dialog::keys::action_text_color(critical, crate::ffi::nr_appearance_is_dark());
    crate::dialog::layout::alert(
        message,
        detail,
        buttons,
        crate::strings::text(language(), crate::strings::key::UPDATE_OK),
        return_button,
        width,
        height,
        color,
        &mut crate::text::typeset::width,
    )
}

/// The share of its time the pairing dialog's code still has, for the line drawn around it.
/// Nothing once a phone has used the code or it has gone stale; the shell then closes the
/// dialog as Escape would.
#[no_mangle]
pub extern "C" fn nr_pair_left() -> f64 {
    crate::remote::app::invite_left()
}

/// Whether the dialog's action is enabled with `typed` in its field, since a name prompt needs
/// a name (`menus::name_of`).
///
/// # Safety
/// `typed` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_prompt_takes(typed: *const c_char) -> bool {
    // SAFETY: the caller passes a NUL-terminated string.
    let typed = unsafe { text_of(typed) };
    let prompt = PROMPT.relock();
    prompt
        .as_ref()
        .is_some_and(|(_, prompt)| prompt.value.is_none())
        || menus::name_of(&typed).is_some()
}

/// Handles the dialog's answer: its action (`taken`, with `typed` in the field) or Cancel.
/// Returns 17 when it stays open because a name is required; otherwise the dialog is closed and
/// the effect is the drive change or a notice of why it was refused.
///
/// # Safety
/// `typed` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_prompt_answer(taken: bool, typed: *const c_char) -> NrMenuEffect {
    // SAFETY: the caller passes a NUL-terminated string.
    let typed = unsafe { text_of(typed) };
    let mut prompt = PROMPT.relock();
    let Some((ask, shown)) = prompt.clone() else {
        return hand_over(Effect::Nothing);
    };
    if matches!(ask, Ask::Pair) {
        crate::remote::app::withdraw_invite();
    }
    if !taken {
        *prompt = None;
        return hand_over(Effect::Nothing);
    }
    if let Ask::Unlock(path) = &ask {
        // A password is taken as typed, spaces and all.
        if typed.is_empty() {
            return hand_over(Effect::Stays);
        }
        *prompt = None;
        crate::pdf::passwords::remember(path, typed.as_bytes());
        *UNLOCK_ASKED.relock() = None;
        crate::deck::ffi::forget_read();
        return hand_over(Effect::Changed);
    }
    let name = menus::name_of(&typed);
    if shown.value.is_some() && name.is_none() {
        return hand_over(Effect::Stays);
    }
    *prompt = None;
    drop(prompt);
    hand_over(do_asked(ask, &name.unwrap_or_default()))
}

/// Does what a confirmed dialog asked, `name` being what was typed in its field.
fn do_asked(ask: Ask, name: &str) -> Effect {
    match ask {
        Ask::Rename(column, id) => {
            changed(with_columns(|columns| columns.rename_id(column, &id, name)))
        }
        Ask::EndSession(id) => {
            crate::ffi::end_session(&id);
            changed(None)
        }
        Ask::ForgetPhone(key) => {
            if let Err(why) = crate::remote::app::forget(&key) {
                eprintln!("phone link\ta phone was not forgotten: {why}");
            }
            changed(None)
        }
        Ask::Quit => Effect::Quit,
        Ask::Unlock(_) | Ask::Pair => changed(None),
    }
}

/// Asks for the open file's password when it is locked (`deck::ReadError::Locked`), saying so
/// when the one last typed didn't open it. Nothing when it isn't locked.
#[no_mangle]
pub extern "C" fn nr_prompt_unlock() -> NrMenuEffect {
    let Some(path) = crate::deck::ffi::opened_locked() else {
        return hand_over(Effect::Nothing);
    };
    // Once a file: not again on each redraw, nor after Cancel, until a password is typed or
    // the file opens anew (`nr_unlock_forget_asked`).
    let mut asked = UNLOCK_ASKED.relock();
    if asked.as_ref() == Some(&path) {
        return hand_over(Effect::Nothing);
    }
    *asked = Some(path.clone());
    drop(asked);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let wrong = crate::pdf::passwords::of(&path).is_some();
    hand_over(ask(
        Ask::Unlock(path),
        menus::unlock_prompt(&name, wrong, language()),
    ))
}

/// Shows a phone the pairing link as a QR code (`menus::pair_prompt`), or says why the link
/// could not start.
fn connect_phone() -> Effect {
    let code = crate::remote::app::invite()
        .and_then(|link| crate::remote::code::modules(&link).ok_or_else(String::new));
    match code {
        Ok(code) => ask(Ask::Pair, menus::pair_prompt(code, language())),
        Err(why) => Effect::Toast(menus::pair_failed(language()), why),
    }
}

/// Asks before forgetting the paired phone with `key` (`menus::forget_phone_prompt`). Nothing
/// when no such phone is paired now.
fn ask_to_forget_phone(key: crate::remote::devices::Key) -> Effect {
    let phones = crate::remote::app::devices();
    match phones.iter().find(|(paired, _)| *paired == key) {
        Some((_, name)) => ask(
            Ask::ForgetPhone(key),
            menus::forget_phone_prompt(name, language()),
        ),
        None => Effect::Nothing,
    }
}

/// Opens the pairing dialog for a check's `phone` step. Whether it is ready to show.
pub(crate) fn prompt_pairing() -> bool {
    matches!(connect_phone(), Effect::Prompt)
}

/// Quits, or confirms first while sessions are working or waiting (`menus::quit_prompt`).
fn quit_or_ask() -> Effect {
    match menus::quit_prompt(crate::ffi::sessions_at_work(), language()) {
        Some(prompt) => ask(Ask::Quit, prompt),
        None => Effect::Quit,
    }
}

/// Whether quitting now needs confirmation, for the window's close button, which uses the same
/// dialog (`nr_quit_or_ask`).
#[no_mangle]
pub extern "C" fn nr_quit_asks() -> bool {
    crate::ffi::sessions_at_work() > 0
}

/// Quits as the menu bar's Quit does: the effect is Quit, or a confirmation dialog.
#[no_mangle]
pub extern "C" fn nr_quit_or_ask() -> NrMenuEffect {
    hand_over(quit_or_ask())
}

/// Ends terminal file `id`'s session, confirming first while it is working or waiting
/// (`menus::end_session_prompt`). The file stays; tabs showing it close.
fn end_session_or_ask(id: &str) -> Effect {
    let Some(activity) = crate::ffi::session_activity(id) else {
        return Effect::Nothing;
    };
    let name = with_columns(|columns| columns.workspace().place_of(id).ok())
        .flatten()
        .map(|placed| placed.title)
        .unwrap_or_default();
    match menus::end_session_prompt(&name, activity, language()) {
        Some(prompt) => ask(Ask::EndSession(id.to_owned()), prompt),
        None => {
            crate::ffi::end_session(id);
            changed(None)
        }
    }
}

// MARK: The drive's edits

/// A column's table number as `nr_rows_key` numbers tables. The list's command folder
/// (`usize::MAX`, `Columns::list_command_column`) maps to the list.
fn table_number(column: usize) -> i64 {
    i64::try_from(column).unwrap_or(-1)
}

/// A drive command from a key or the Edit menu while a drive view has focus.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrDriveEdit {
    /// ⌘C: copies the selected files to the clipboard (`NrEffect::CopyFiles`).
    Copy,
    /// ⌘D: duplicates each selected file beside itself.
    Duplicate,
    /// ⌘Z: undoes the newest drive change made in the app.
    Undo,
    /// ⇧⌘Z: redoes the last undone change.
    Redo,
    /// ⌘⌫ or Delete in a table (`NrRowIntent::Trash`): moves the selected files to the Trash at
    /// once.
    Trash,
}

/// The files selected in `table`. The board selects one card at a time, tracked by the board
/// (`board::ffi`) and not its column, so while the board shows, the shown column's number
/// returns the board's selected card.
fn selected_ids(table: Table) -> Vec<String> {
    let board_shows = with_columns(|columns| {
        columns.view() == View::Board
            && table == Table::Column(columns.columns().len().saturating_sub(1))
    })
    .unwrap_or(false);
    if board_shows {
        return crate::board::ffi::board_selected_id().into_iter().collect();
    }
    with_columns(|columns| columns.selected_ids(table)).unwrap_or_default()
}

/// The effect of an undo or redo: a redraw, a notice of why it was refused, or nothing when
/// there was nothing to do.
fn history_changed(outcome: Option<DriveResult<()>>) -> Effect {
    match outcome {
        Some(outcome) => changed(Some(outcome)),
        None => Effect::Nothing,
    }
}

/// Applies `change` to `table`'s selected files; does nothing when none is selected.
fn change_selected(table: Table, change: fn(&mut Columns, &[String]) -> DriveResult<()>) -> Effect {
    let ids = selected_ids(table);
    if ids.is_empty() {
        return Effect::Nothing;
    }
    changed(with_columns(|columns| change(columns, &ids)))
}

/// Runs drive command `edit` for `table`'s selection, with tables numbered as in `nr_rows_key`:
/// -1 for the list, n ≥ 0 for column n (the board uses its own selected card; see
/// `selected_ids`). Copy returns `CopyFiles` with the selection's paths, or `Nothing` when
/// empty. Duplicate, Undo, Redo and Trash run here and return `Changed`, `Toast` with the
/// drive's refusal, or `Nothing` when there was nothing to do.
#[no_mangle]
pub extern "C" fn nr_drive_edit(edit: NrDriveEdit, table: i64) -> NrMenuEffect {
    let table = crate::window::ffi::table(table);
    hand_over(match edit {
        NrDriveEdit::Copy => with_paths(&selected_ids(table), Effect::CopyFiles),
        NrDriveEdit::Duplicate => change_selected(table, Columns::duplicate),
        NrDriveEdit::Trash => change_selected(table, Columns::trash),
        NrDriveEdit::Undo => history_changed(with_columns(Columns::undo).flatten()),
        NrDriveEdit::Redo => history_changed(with_columns(Columns::redo).flatten()),
    })
}

/// Copies the newline-joined `paths` into `table`'s folder (numbered as in `nr_drive_edit`; the
/// list pastes into the folder its selected files share, else the shown folder), recorded as
/// one undoable change. Folders are copied recursively; a name clash gets a number.
///
/// # Safety
/// `paths` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_drive_paste(table: i64, paths: *const c_char) -> NrMenuEffect {
    // SAFETY: the caller passes a NUL-terminated string.
    let sources: Vec<PathBuf> = crate::ffi::ids_of(&unsafe { text_of(paths) })
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if sources.is_empty() {
        return hand_over(Effect::Nothing);
    }
    let table = crate::window::ffi::table(table);
    hand_over(changed(with_columns(|columns| {
        columns.paste(table, &sources)
    })))
}

// MARK: Compress

/// Effects left for the shell by background drive work, oldest first, such as a failed
/// Compress notice (`nr_drive_work_take`).
static WORK_LEFT: Mutex<VecDeque<Effect>> = Mutex::new(VecDeque::new());

/// Compresses files `ids` into one zip archive beside the first (`archive`) on a background
/// thread, so the window stays responsive. Refused at once where that folder accepts no new
/// files. When complete, the drive lists the archive and records it as an undoable change; a
/// failure leaves nothing and queues a notice for the shell (`nr_drive_work_take`).
fn start_compress(ids: &[String]) -> Effect {
    let plan = match with_columns(|columns| columns.plan_compress(ids, language())) {
        Some(Ok(Some(plan))) => plan,
        Some(Err(error)) => return changed(Some(Err(error))),
        Some(Ok(None)) | None => return Effect::Nothing,
    };
    let started = std::thread::Builder::new()
        .name("compress".to_owned())
        .spawn(move || {
            let written = plan.run();
            finish_compress(&plan, written);
        });
    match started {
        Ok(_) => Effect::Nothing,
        Err(cause) => changed(Some(Err(DriveError::Failed(format!(
            "start compressing: {cause}"
        ))))),
    }
}

/// Handles a finished Compress on its thread: records and lists the archive, or queues a notice
/// of why it failed, then wakes the shell.
fn finish_compress(plan: &crate::archive::Compress, written: std::io::Result<PathBuf>) {
    let outcome = match written {
        Ok(archive) => with_columns(|columns| columns.record_made(archive)),
        Err(cause) => Some(Err(DriveError::Failed(format!(
            "compress {}: {cause}",
            plan.name
        )))),
    };
    let effect = changed(outcome);
    if matches!(effect, Effect::Toast(..)) {
        WORK_LEFT.relock().push_back(effect);
    }
    crate::transport::wake();
}

/// Takes the oldest effect left by background drive work: `Toast` with why a Compress failed,
/// or `Nothing` once the queue is empty. The shell calls it whenever the core wakes it
/// (`nr_transport_open`).
#[no_mangle]
pub extern "C" fn nr_drive_work_take() -> NrMenuEffect {
    let left = WORK_LEFT.relock().pop_front();
    hand_over(left.unwrap_or(Effect::Nothing))
}

// MARK: Drops

/// Handles drive files `ids` (one per line) dropped on file `target` in a column's folder. A
/// folder receives them by move; a running terminal that accepts drops receives their paths
/// and stays put. Anything else accepts nothing.
///
/// # Safety
/// `ids` and `target` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_folder_drop(
    column: usize,
    ids: *const c_char,
    target: *const c_char,
) -> NrMenuEffect {
    // SAFETY: the caller passes NUL-terminated strings.
    let (ids, target) = unsafe { (crate::ffi::ids_of(&text_of(ids)), text_of(target)) };
    let Some(entry) = entries(column, std::slice::from_ref(&target))
        .into_iter()
        .next()
    else {
        return hand_over(Effect::Nothing);
    };
    if entry.kind == EntryKind::Folder {
        return hand_over(changed(with_columns(|columns| {
            columns.move_ids(column, &ids, Some(&target))
        })));
    }
    if !crate::ffi::file_row(&entry, &crate::ffi::running_sessions()).takes_references {
        return hand_over(Effect::Nothing);
    }
    if crate::ffi::reference_files(&target, &ids) {
        return hand_over(Effect::Changed);
    }
    hand_over(Effect::Toast(
        menus::add_failed(language()),
        menus::add_failed_detail(&entry.title, language()),
    ))
}

/// Handles a drive change's result, `refusal` (empty on success, as the file operations report
/// it): a redraw, plus a notice of why when refused.
///
/// # Safety
/// `refusal` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_refused(refusal: *const c_char) -> NrMenuEffect {
    // SAFETY: the caller passes a NUL-terminated string.
    let refusal = unsafe { text_of(refusal) };
    hand_over(if refusal.is_empty() {
        Effect::Changed
    } else {
        Effect::Toast(menus::update_failed(language()), refusal)
    })
}

/// Notice timing (`dialog::keys::TOAST_SECONDS`): how long one stays, and how many show at once
/// (a new one replaces the one showing).
#[repr(C)]
pub struct NrToastPolicy {
    pub seconds: f64,
    pub at_once: usize,
}

#[no_mangle]
pub extern "C" fn nr_toast_policy() -> NrToastPolicy {
    NrToastPolicy {
        seconds: crate::dialog::keys::TOAST_SECONDS,
        at_once: crate::dialog::keys::TOASTS_AT_ONCE,
    }
}

/// Layout of a notice with `title` and `detail` in a `width` by `height` window (from its
/// top-left), with its lines relative to the notice (`dialog::layout::ToastLayout`).
pub fn toast_layout(
    title: &str,
    detail: &str,
    width: f64,
    height: f64,
) -> crate::dialog::layout::ToastLayout {
    crate::dialog::layout::toast(
        title,
        detail,
        width,
        height,
        &mut crate::text::typeset::width,
    )
}

// MARK: A dialog's keys

static DIALOGS: Mutex<[Option<Focus>; 4]> = Mutex::new([None; 4]);

fn with_dialog<T>(dialog: Dialog, action: impl FnOnce(&mut Focus) -> T) -> Option<T> {
    DIALOGS.relock()[dialog as usize].as_mut().map(action)
}

/// A press focused control `part` of `dialog`, without the keyboard focus ring.
pub(crate) fn point_dialog(dialog: Dialog, part: usize) {
    with_dialog(dialog, |focus| focus.point(part));
}

/// Dialog `dialog` opened, by a key when `by_key`. Its initial focus is what `nr_dialog_focus`
/// reports.
#[no_mangle]
pub extern "C" fn nr_dialog_open(dialog: NrDialog, by_key: bool) {
    let dialog = dialog.dialog();
    DIALOGS.relock()[dialog as usize] = Some(Focus::open(dialog, by_key));
}

#[no_mangle]
pub extern "C" fn nr_dialog_close(dialog: NrDialog) {
    DIALOGS.relock()[dialog.dialog() as usize] = None;
}

/// Which dialog control has focus (`part`, -1 for none) and whether its ring shows.
#[repr(C)]
pub struct NrDialogFocus {
    pub part: i64,
    pub ring: bool,
}

#[no_mangle]
pub extern "C" fn nr_dialog_focus(dialog: NrDialog) -> NrDialogFocus {
    let focus = with_dialog(dialog.dialog(), |focus| *focus);
    NrDialogFocus {
        part: focus.map_or(-1, |focus| focus.focused as i64),
        ring: focus.is_some_and(|focus| focus.ring),
    }
}

/// A press focused control `part` of `dialog`, without the keyboard focus ring.
#[no_mangle]
pub extern "C" fn nr_dialog_point(dialog: NrDialog, part: usize) {
    point_dialog(dialog.dialog(), part);
}

/// The alert's Cancel button as a `part` (`dialog::keys::Dialog::Alert`).
pub const NR_ALERT_CANCEL: usize = 0;
/// The alert's action.
pub const NR_ALERT_ACTION: usize = 1;

/// What a key did in a dialog.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrDialogIntent {
    /// The dialog ignores it; pass it to the focused control.
    PassOn,
    /// Focus moved to `part`, with its ring.
    Focus,
    /// Press `part`, which performs the dialog's action (`takes`) or acts as Cancel.
    Press,
    /// Close the dialog as Cancel does.
    Close,
}

/// What a key did in a dialog, and the part involved.
#[repr(C)]
pub struct NrDialogKey {
    pub intent: NrDialogIntent,
    pub part: usize,
    pub takes: bool,
}

/// Handles a key in `dialog`. Tab (⇧Tab), Return, Space, Escape, ↓ and ↑ belong to the dialog.
#[no_mangle]
pub extern "C" fn nr_dialog_key(dialog: NrDialog, press: NrKeyPress) -> NrDialogKey {
    use crate::dialog::keys::{Intent, Key};
    let key = match press.key {
        NrKey::Tab if press.shift() => Key::BackTab,
        NrKey::Tab => Key::Tab,
        NrKey::Return => Key::Return,
        NrKey::Space => Key::Space,
        NrKey::Escape => Key::Escape,
        NrKey::Down => Key::Down,
        NrKey::Up => Key::Up,
        _ => Key::Other,
    };
    let dialog = dialog.dialog();
    let intent = with_dialog(dialog, |focus| focus.key(key)).unwrap_or(Intent::PassOn);
    let (intent, part) = match intent {
        Intent::PassOn => (NrDialogIntent::PassOn, 0),
        Intent::Focus(part) => (NrDialogIntent::Focus, part),
        Intent::Press(part) => (NrDialogIntent::Press, part),
        Intent::Close => (NrDialogIntent::Close, 0),
    };
    NrDialogKey {
        intent,
        part,
        takes: intent == NrDialogIntent::Press && dialog.takes(part),
    }
}

// MARK: A drawn menu's rows

/// A drawn menu row's kind (`menus::layout::Kind`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMenuRowKind {
    Item,
    Label,
    Separator,
}

/// A drawn menu row (`menus::layout::Row`). `shortcut` is the text beside the label, empty for
/// none; `image` means a glyph or image precedes the label; `ticked` reserves tick space (a
/// tick, a dash, or a toggle); `tight` follows the row above with no gap; `detail` is muted
/// text on a second line, empty for none.
#[repr(C)]
pub struct NrMenuItem {
    pub kind: NrMenuRowKind,
    pub title: *const c_char,
    pub shortcut: *const c_char,
    pub detail: *const c_char,
    pub image: bool,
    pub ticked: bool,
    pub submenu: bool,
    pub enabled: bool,
    pub tight: bool,
}

/// A drawn menu's style (`menus::layout::Style`, from `menus::Menu`): `max_height` is 0 for no
/// limit, and `quiet_labels` makes labels muted headings for the rows under them.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrMenuStyle {
    pub min_width: f64,
    pub max_height: f64,
    pub icon_size: f64,
    pub icon_gap: f64,
    pub quiet_labels: bool,
}

/// A row's part frames (`menus::layout::Place`); a missing part is an empty frame. `words` and
/// `keys` are line boxes (top, height and maximum text width); a shortcut starts at `keys`'
/// left.
#[repr(C)]
pub struct NrMenuRowPlace {
    pub frame: NrRect,
    pub selectable: bool,
    pub check: NrRect,
    pub icon: NrRect,
    pub caret: NrRect,
    pub words: NrRect,
    pub words_font: crate::board::ffi::NrCardFont,
    pub keys: NrRect,
    pub keys_font: crate::board::ffi::NrCardFont,
    /// An item's second line; empty for a row without one.
    pub detail: NrRect,
    pub detail_font: crate::board::ffi::NrCardFont,
}

#[repr(C)]
pub struct NrMenuLayout {
    pub width: f64,
    pub height: f64,
    pub content_height: f64,
    pub places: *const NrMenuRowPlace,
    pub count: usize,
}

/// Lays out a drawn menu of `count` `items` in `style`, measuring text with the core's
/// typesetter (`typeset`). The results are valid until the next call.
///
/// # Safety
/// `items` must point to `count` rows whose texts are NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_menu_layout(
    items: *const NrMenuItem,
    count: usize,
    style: NrMenuStyle,
) -> NrMenuLayout {
    use crate::menus::layout::{layout, Kind, Row, Style};
    static PLACES: Mutex<Vec<NrMenuRowPlace>> = Mutex::new(Vec::new());
    let items: &[NrMenuItem] = if items.is_null() || count == 0 {
        &[]
    } else {
        // SAFETY: the caller passes `count` rows.
        unsafe { std::slice::from_raw_parts(items, count) }
    };
    let rows: Vec<Row> = items
        .iter()
        .map(|item| Row {
            kind: match item.kind {
                NrMenuRowKind::Item => Kind::Item,
                NrMenuRowKind::Label => Kind::Label,
                NrMenuRowKind::Separator => Kind::Separator,
            },
            // SAFETY: the caller passes NUL-terminated strings.
            title: unsafe { text_of(item.title) },
            shortcut: unsafe { text_of(item.shortcut) },
            // SAFETY: the same.
            detail: unsafe { text_of(item.detail) },
            image: item.image,
            ticked: item.ticked,
            submenu: item.submenu,
            enabled: item.enabled,
            tight: item.tight,
        })
        .collect();
    let style = Style {
        min_width: style.min_width,
        max_height: style.max_height,
        icon_size: style.icon_size,
        icon_gap: style.icon_gap,
        quiet_labels: style.quiet_labels,
    };
    let placed = layout(&rows, &style, &mut |text, font| {
        crate::text::typeset::width(text, font)
    });
    let rect = |frame: crate::board::Frame| NrRect {
        x: frame.x,
        y: frame.y,
        width: frame.width,
        height: frame.height,
    };
    let font = |font: crate::board::card::Font| crate::board::ffi::NrCardFont {
        size: font.size,
        weight: font.weight,
        mono: font.mono,
        slanted: font.slanted,
        tabular: font.tabular,
    };
    let mut places = PLACES.relock();
    *places = placed
        .places
        .iter()
        .map(|place| NrMenuRowPlace {
            frame: rect(place.frame),
            selectable: place.selectable,
            check: rect(place.check),
            icon: rect(place.icon),
            caret: rect(place.caret),
            words: rect(place.words),
            words_font: font(place.words_font),
            keys: rect(place.keys),
            keys_font: font(place.keys_font),
            detail: rect(place.detail),
            detail_font: font(crate::menus::layout::DETAIL_FONT),
        })
        .collect();
    NrMenuLayout {
        width: placed.width,
        height: placed.height,
        content_height: placed.content_height,
        places: places.as_ptr(),
        count: places.len(),
    }
}

// MARK: A drawn menu's keys and place

/// A row of the deepest open drawn menu, for key handling.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct NrMenuRow {
    pub selectable: bool,
    pub has_submenu: bool,
}

fn rows_of(rows: *const NrMenuRow, count: usize) -> Vec<crate::menus::keys::Row> {
    if rows.is_null() || count == 0 {
        return Vec::new();
    }
    // SAFETY: the caller passes `count` rows.
    let rows: &[NrMenuRow] = unsafe { std::slice::from_raw_parts(rows, count) };
    rows.iter()
        .map(|row| crate::menus::keys::Row {
            selectable: row.selectable,
            has_submenu: row.has_submenu,
        })
        .collect()
}

/// What a key did in an open drawn menu.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrMenuIntent {
    Nothing,
    /// Every menu closes.
    CloseAll,
    /// Highlight `row`.
    Light,
    /// Open `row`'s submenu now and highlight its first selectable row.
    OpenSubmenu,
    /// Choose `row`.
    Choose,
    /// Close the deepest submenu.
    CloseSubmenu,
}

/// What a key did in an open drawn menu, and the row involved.
#[repr(C)]
pub struct NrMenuKey {
    pub intent: NrMenuIntent,
    pub row: usize,
}

/// Handles a key while a drawn menu is open; Escape, the arrows and Return belong to the menu.
/// `rows` are the deepest menu's `count` rows, `lit` the highlighted one (-1 for none), and
/// `depth` the number of menus open under it.
///
/// # Safety
/// `rows` must point to `count` rows.
#[no_mangle]
pub unsafe extern "C" fn nr_menu_key(
    press: NrKeyPress,
    rows: *const NrMenuRow,
    count: usize,
    lit: i64,
    depth: usize,
) -> NrMenuKey {
    use crate::menus::keys::{key_intent, Intent, Key};
    let key = match press.key {
        NrKey::Escape => Key::Escape,
        NrKey::Down => Key::Down,
        NrKey::Up => Key::Up,
        NrKey::Right => Key::Right,
        NrKey::Left => Key::Left,
        NrKey::Return => Key::Return,
        _ => Key::Other,
    };
    let (intent, row) =
        match key_intent(key, &rows_of(rows, count), usize::try_from(lit).ok(), depth) {
            Intent::Nothing => (NrMenuIntent::Nothing, 0),
            Intent::CloseAll => (NrMenuIntent::CloseAll, 0),
            Intent::Light(row) => (NrMenuIntent::Light, row),
            Intent::OpenSubmenu(row) => (NrMenuIntent::OpenSubmenu, row),
            Intent::Choose(row) => (NrMenuIntent::Choose, row),
            Intent::CloseSubmenu => (NrMenuIntent::CloseSubmenu, 0),
        };
    NrMenuKey { intent, row }
}

/// The row highlighted when a submenu opens from the keyboard, or -1.
///
/// # Safety
/// `rows` must point to `count` rows.
#[no_mangle]
pub unsafe extern "C" fn nr_menu_first_row(rows: *const NrMenuRow, count: usize) -> i64 {
    crate::menus::keys::first_selectable(&rows_of(rows, count)).map_or(-1, |row| row as i64)
}

/// How hovering a row affects the submenus (`menus::keys::hover`).
#[repr(C)]
pub struct NrMenuHover {
    pub close_deeper: bool,
    pub open: bool,
}

/// The pointer is over a drawn menu row (`over`; `row` and `enabled` describe it) or over none.
/// `deeper_open` says a submenu is open under this menu, and `deeper_is_rows` that it belongs
/// to this row.
#[no_mangle]
pub extern "C" fn nr_menu_hover(
    over: bool,
    row: NrMenuRow,
    enabled: bool,
    deeper_open: bool,
    deeper_is_rows: bool,
) -> NrMenuHover {
    let row = over.then_some(crate::menus::keys::Row {
        selectable: row.selectable,
        has_submenu: row.has_submenu,
    });
    let hover = crate::menus::keys::hover(row, enabled, deeper_open, deeper_is_rows);
    NrMenuHover {
        close_deeper: hover.close_deeper,
        open: hover.open,
    }
}

/// Whether a highlighted row stays highlighted as the pointer leaves its menu.
#[no_mangle]
pub extern "C" fn nr_menu_stays_lit(has_submenu: bool) -> bool {
    crate::menus::keys::stays_lit_on_exit(has_submenu)
}

/// Where a drawn menu opens (`menus::keys::place`): its lower-left corner, its slide-in direction
/// (in units of `nr_motion_number(Slide)`), and the point it grows from.
#[repr(C)]
pub struct NrMenuPlace {
    pub x: f64,
    pub y: f64,
    pub slide_x: f64,
    pub slide_y: f64,
    pub origin_x: f64,
    pub origin_y: f64,
}

/// Places a `width` × `height` menu `gap` from `anchor` inside `bounds`, or hanging `below` its
/// control, snapped to device pixels at `scale` per point. Rectangles have y increasing upwards.
#[no_mangle]
pub extern "C" fn nr_menu_place(
    anchor: NrRect,
    width: f64,
    height: f64,
    bounds: NrRect,
    gap: f64,
    below: bool,
    scale: f64,
) -> NrMenuPlace {
    use crate::menus::keys::Rect;
    let rect = |rect: NrRect| Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    };
    let placed =
        crate::menus::keys::place(rect(anchor), width, height, rect(bounds), gap, below, scale);
    NrMenuPlace {
        x: placed.x,
        y: placed.y,
        slide_x: placed.slide_x,
        slide_y: placed.slide_y,
        origin_x: placed.origin_x,
        origin_y: placed.origin_y,
    }
}

/// The lower edge of a menu redrawn in another's place, keeping its `top` and no lower than
/// `bottom`.
#[no_mangle]
pub extern "C" fn nr_menu_replacing_y(top: f64, height: f64, bottom: f64) -> f64 {
    crate::menus::keys::place_replacing(0.0, top, height, bottom).1
}

/// Highlight opacity for the hovered row (`menus::keys::light`).
#[no_mangle]
pub extern "C" fn nr_menu_light(destructive: bool, dark: bool) -> f64 {
    crate::menus::keys::light(destructive, dark)
}

// MARK: The list header

/// The header as last laid out, with its column labels.
struct Header {
    layout: crate::list::header::Layout,
    labels: Vec<&'static str>,
    right_aligned: Vec<bool>,
}

static HEADER: Mutex<Option<Header>> = Mutex::new(None);
static HEADER_PRESS: Mutex<Option<(crate::list::header::Layout, crate::list::header::Press)>> =
    Mutex::new(None);

fn header_layout(width: f64) -> Header {
    use crate::list::Column;
    let columns: Vec<(Column, f64)> = with_columns(|columns| {
        columns
            .list_columns()
            .iter()
            .skip(1)
            .map(|column| (*column, columns.list_width(*column)))
            .collect()
    })
    .unwrap_or_default();
    let placed: Vec<(u32, f64)> = columns
        .iter()
        .map(|(column, width)| (column.number(), *width))
        .collect();
    Header {
        layout: crate::list::header::lay_out(width, &placed),
        labels: columns.iter().map(|(column, _)| column.label()).collect(),
        right_aligned: columns
            .iter()
            .map(|(column, _)| *column == Column::Size)
            .collect(),
    }
}

/// The list columns across a `width`-wide row (`list::header::lay_out`): Name's span and label,
/// the options button at its end, the number of following columns (`nr_list_header_column`),
/// and the resize hit area either side of a column edge.
#[repr(C)]
pub struct NrListHeader {
    pub name_x: f64,
    pub name_width: f64,
    pub name_label: NrText,
    pub chooser: NrRect,
    pub count: usize,
    pub edge_reach: f64,
}

#[no_mangle]
pub extern "C" fn nr_list_header_lay_out(width: f64) -> NrListHeader {
    let header = header_layout(width);
    let chooser = header.layout.chooser();
    let laid = NrListHeader {
        name_x: header.layout.name_x,
        name_width: header.layout.name_width,
        name_label: NrText::borrowing(crate::list::Column::Name.label()),
        chooser: NrRect {
            x: chooser.x,
            y: chooser.y,
            width: chooser.width,
            height: chooser.height,
        },
        count: header.layout.others.len(),
        edge_reach: crate::list::header::EDGE_REACH,
    };
    *HEADER.relock() = Some(header);
    laid
}

/// A column after Name as last laid out: its key, name, span, the resize edge before it, and
/// whether its cells are right-aligned numbers.
#[repr(C)]
pub struct NrListPlaced {
    pub key: u32,
    pub label: NrText,
    pub x: f64,
    pub width: f64,
    pub edge: f64,
    pub right_aligned: bool,
}

/// Fills `out` with column `index` after Name as `nr_list_header_lay_out` last placed it.
///
/// # Safety
/// `out` must point to writable storage for one `NrListPlaced`.
#[no_mangle]
pub unsafe extern "C" fn nr_list_header_column(index: usize, out: *mut NrListPlaced) -> bool {
    let header = HEADER.relock();
    let Some((header, placed)) = header.as_ref().and_then(|header| {
        header
            .layout
            .others
            .get(index)
            .map(|placed| (header, *placed))
    }) else {
        return false;
    };
    // SAFETY: the caller passes storage for one column; the labels are static.
    unsafe {
        out.write(NrListPlaced {
            key: placed.key,
            label: NrText::borrowing(header.labels[index]),
            x: placed.x,
            width: placed.width,
            edge: placed.edge(),
            right_aligned: header.right_aligned[index],
        });
    }
    true
}

/// The result of a press on the list header.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrHeaderPress {
    Nothing,
    /// The list changed (a width reset, a sort) and is redrawn.
    Changed,
    /// Open the options under the button.
    Options,
    /// Track the press: send its moves to `nr_list_header_drag` and its release to
    /// `nr_list_header_release`.
    Follow,
}

/// A press at `x`, `y` on a `width`-wide header; `clicks` counts consecutive clicks.
#[no_mangle]
pub extern "C" fn nr_list_header_press(width: f64, x: f64, y: f64, clicks: u32) -> NrHeaderPress {
    use crate::list::header::{press, Pressed};
    let layout = header_layout(width).layout;
    match press(&layout, x, y, clicks) {
        Pressed::Nothing => NrHeaderPress::Nothing,
        Pressed::ResetWidth(key) => {
            crate::ffi::list_set_width(key, 0.0);
            NrHeaderPress::Changed
        }
        Pressed::Options => NrHeaderPress::Options,
        Pressed::Sort(key) => {
            crate::ffi::list_toggle_sort(key);
            NrHeaderPress::Changed
        }
        Pressed::Follow(followed) => {
            *HEADER_PRESS.relock() = Some((layout, followed));
            NrHeaderPress::Follow
        }
    }
}

/// The tracked press moved to `x`. True when the list changed (a column width).
#[no_mangle]
pub extern "C" fn nr_list_header_drag(x: f64) -> bool {
    let resized = HEADER_PRESS
        .relock()
        .as_mut()
        .and_then(|(_, press)| press.drag(x));
    resized
        .inspect(|(key, width)| crate::ffi::list_set_width(*key, *width))
        .is_some()
}

/// The tracked press was released at `x`: a resize ends, a dragged column moves, and an
/// undragged one sorts. True when the list changed.
#[no_mangle]
pub extern "C" fn nr_list_header_release(x: f64) -> bool {
    use crate::list::header::Released;
    nr_list_header_drag(x);
    let Some((layout, press)) = HEADER_PRESS.relock().take() else {
        return false;
    };
    match press.release(&layout) {
        Released::Resized => {}
        Released::Sort(key) => crate::ffi::list_toggle_sort(key),
        Released::Move(key, place) => crate::ffi::list_move_column(key, place),
    }
    true
}

/// The column under a right click at `x` on a `width`-wide header, for its Move items, or -1.
#[no_mangle]
pub extern "C" fn nr_list_header_menu_column(width: f64, x: f64) -> i64 {
    header_layout(width)
        .layout
        .menu_column(x)
        .map_or(-1, i64::from)
}
