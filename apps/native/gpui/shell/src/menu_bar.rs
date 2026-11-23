//! The app menu bar: the core's menus (`menus::ffi::menu_bar`) set as native menus through GPUI's
//! `set_menus` (the real menu bar on a Mac), item for item and key for key. Core commands run
//! through `nr_menu_run` and their effects through `effects::carry_out`. System commands (About,
//! Hide, Minimize…, `menus::Role`) belong to the platform. Text commands (Undo, Redo, Cut, Copy,
//! Paste, Paste and Match Style, Delete, Select All, Start and Stop Speaking) go to the focused
//! field, as AppKit sends them to the first responder, and are enabled only while one takes them.
//! AppKit adds its own Edit items (Writing Tools, AutoFill, Start Dictation, Emoji & Symbols).
//!
//! The Northroom status icon (`tray`) holds the core's tray menu (`menus::ffi::menu_tray`).
//!
//! Native items freeze their title, tick and enabled state when set, so `refresh` resets the
//! menus whenever the core's description changes. Entries the core hides become hidden items on
//! a Mac that keep their shortcuts, so keys a native view (a live page) passes on still reach
//! them through AppKit. Elsewhere they are left out and their keys work through the window
//! (`keys`).

use crate::effects;
use crate::workspace::Workspace;
use gpui::{
    Action, App, Global, KeyBinding, KeyBindingContextPredicate, Menu, MenuItem, OsAction,
    SystemMenuType, Window, WindowHandle,
};
use kit::controls::input;
use kit::core::c;
use kit::lists::rename::Said;
use kit::menu_model::{Check, Kind};
use kit::nr::*;
use northroom_core::menus::Role;
pub use northroom_core::menus::{Entry as BarEntry, Menu as BarMenu};
use std::rc::Rc;

/// Whether `entry` is one of the platform's own commands.
pub fn is_system(entry: &BarEntry) -> bool {
    entry.role != Role::None
}

pub fn menus() -> Vec<BarMenu> {
    menu_bar()
}

/// A chosen core menu command, by entry id.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = northroom, no_json)]
pub struct RunCommand {
    pub id: String,
}

/// A core command that is disabled right now. Nothing handles it, so the platform greys it out.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = northroom, no_json)]
pub struct CommandOff {
    pub id: String,
}

/// A platform command the app carries out itself (`menus::Role`).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = northroom, no_json)]
pub struct SystemCommand {
    pub role: Role,
}

/// Key context for the menu bar's bindings. No view has it, so the bindings only label shortcuts
/// beside items; the key itself goes to the views, then the window (`keys::window_key`).
const SHOWN_ONLY: &str = "NorthroomMenuBar";

/// The menus as last set.
#[derive(Default)]
struct Shown {
    menus: Vec<BarMenu>,
}

impl Global for Shown {}

/// Sets up the menu bar, its command handlers and shortcut labels. Call once at launch.
pub fn install(handle: WindowHandle<Workspace>, cx: &mut App) {
    cx.on_action(move |command: &RunCommand, cx| {
        let id = c(&command.id);
        // SAFETY: a NUL-terminated string.
        let said = Said::of(&unsafe { nr_menu_run(NrMenu::Bar, id.as_ptr()) });
        let _ = handle.update(cx, |workspace, window, cx| {
            effects::carry_out(workspace, said, window, cx)
        });
    });
    cx.on_action(move |command: &SystemCommand, cx| {
        let _ = handle.update(cx, |_, window, cx| run_system(command.role, window, cx));
    });
    let mut bindings = Vec::new();
    for menu in menus() {
        named_keys(&menu, &mut bindings, cx);
    }
    cx.bind_keys(bindings);
    cx.set_global(Shown::default());
    refresh(cx);
    // Checks add no status icon, though the tray menu can still be reported. Linux has no tray
    // because nothing in the build speaks StatusNotifierItem.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    if !nr_launch().unattended {
        install_tray(handle, cx);
    }
}

// MARK: Tray

fn tray_menu() -> BarMenu {
    menu_tray()
}

/// Keeps the status icon alive for the life of the app.
#[cfg(any(target_os = "macos", target_os = "windows"))]
struct TrayShown {
    _tray: kit::platform::Tray,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Global for TrayShown {}

/// Adds the Northroom icon to the Mac menu bar or the Windows notification area, with a
/// "Northroom" tooltip and the core's tray menu, rebuilt each time it opens. Rows run through
/// `nr_menu_run`, including Quit, which asks first while sessions are busy.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn install_tray(handle: WindowHandle<Workspace>, cx: &mut App) {
    use kit::platform::{TrayMenu, TrayRow, show_tray};
    use std::cell::RefCell;
    static PICTURE: &[u8] = include_bytes!("../assets/trayTemplate.png");
    static PICTURE_2X: &[u8] = include_bytes!("../assets/trayTemplate@2x.png");
    // The entries the menu last showed, so a chosen row can be looked up.
    let shown: Rc<RefCell<Vec<BarEntry>>> = Rc::default();
    let rows = {
        let shown = shown.clone();
        move || {
            let entries: Vec<BarEntry> = tray_menu()
                .entries
                .into_iter()
                .filter(|entry| !entry.hidden)
                .collect();
            let rows = entries
                .iter()
                .map(|entry| TrayRow {
                    title: entry.title.clone(),
                    separator: entry.kind == Kind::Separator,
                    enabled: is_system(entry) || entry.enabled,
                    ticked: entry.check == Check::On,
                    key: entry.key.to_lowercase(),
                })
                .collect();
            *shown.borrow_mut() = entries;
            rows
        }
    };
    let later = cx.to_async();
    let chosen = move |place: usize| {
        let Some(entry) = shown.borrow().get(place).cloned() else {
            return;
        };
        let _ = handle.update(&mut later.clone(), |workspace, window, cx| {
            if is_system(&entry) {
                run_system(entry.role, window, cx);
                return;
            }
            let id = c(&entry.id);
            // SAFETY: a NUL-terminated string.
            let said = Said::of(&unsafe { nr_menu_run(NrMenu::Tray, id.as_ptr()) });
            effects::carry_out(workspace, said, window, cx);
        });
    };
    let tooltip = kit::core::text(&nr_menu_tray_tooltip());
    let tray = show_tray(
        [PICTURE, PICTURE_2X],
        &tooltip,
        TrayMenu {
            rows: Box::new(rows),
            chosen: Box::new(chosen),
        },
    );
    cx.set_global(TrayShown { _tray: tray });
}

/// Bindings that label the shortcuts of `menu`'s entries.
fn named_keys(menu: &BarMenu, bindings: &mut Vec<KeyBinding>, cx: &App) {
    let context: Rc<KeyBindingContextPredicate> = KeyBindingContextPredicate::parse(SHOWN_ONLY)
        .expect("a context's name is a predicate")
        .into();
    let bind = |keys: &str, action: Box<dyn Action>| {
        KeyBinding::load(keys, action, Some(context.clone()), false, None, cx)
            .expect("the core's shortcuts are keys GPUI reads")
    };
    for entry in &menu.entries {
        if let Some(submenu) = &entry.submenu {
            named_keys(submenu, bindings, cx);
        }
        if entry.key.is_empty() || (entry.hidden && !HIDDEN_KEPT) {
            continue;
        }
        let keys = keystroke(&entry.key, entry.held);
        bindings.extend(
            named_actions(entry)
                .into_iter()
                .map(|action| bind(&keys, action)),
        );
    }
}

/// The actions an entry's shortcut binds to: a system command's own action, or the core command
/// plus its disabled stand-in.
fn named_actions(entry: &BarEntry) -> Vec<Box<dyn Action>> {
    if !is_system(entry) {
        let id = entry.id.clone();
        return vec![
            Box::new(RunCommand { id: id.clone() }),
            Box::new(CommandOff { id }),
        ];
    }
    // Field commands are labelled by the field's own bindings.
    match os_action(entry.role) {
        Some(_) => Vec::new(),
        None => vec![system_action(entry.role)],
    }
}

/// GPUI's keystroke syntax for a key and its modifiers (`cmd-shift-z`).
fn keystroke(key: &str, held: u32) -> String {
    let mut words = Vec::new();
    for (bit, word) in [
        (NR_HELD_CONTROL, "ctrl"),
        (NR_HELD_OPTION, "alt"),
        (NR_HELD_SHIFT, "shift"),
        (NR_HELD_COMMAND, "cmd"),
    ] {
        if held & bit != 0 {
            words.push(word);
        }
    }
    let key = key.to_lowercase();
    words.push(&key);
    words.join("-")
}

/// Resets the menus if the core's description changed since they were last set.
pub fn refresh(cx: &mut App) {
    let described = menu_bar();
    if cx
        .try_global::<Shown>()
        .is_none_or(|shown| shown.menus == described)
    {
        return;
    }
    cx.set_menus(described.iter().map(platform_menu));
    #[cfg(target_os = "macos")]
    {
        use northroom_core::menus::MenuRole;
        kit::platform::name_system_menus(
            described
                .iter()
                .position(|menu| menu.role == MenuRole::Window),
            described
                .iter()
                .position(|menu| menu.role == MenuRole::Help),
        );
        let (mut hidden, mut pasting) = (Vec::new(), Vec::new());
        for (index, menu) in described.iter().enumerate() {
            places_of(menu, vec![index], &|entry| entry.hidden, &mut hidden);
            places_of(menu, vec![index], &pastes, &mut pasting);
        }
        kit::platform::hide_items(&hidden);
        kit::platform::paste_items(&pasting);
    }
    cx.set_global(Shown { menus: described });
}

/// Whether hidden entries stay in the menu as hidden items that keep their shortcuts. Only AppKit
/// supports this.
const HIDDEN_KEPT: bool = cfg!(target_os = "macos");

fn platform_menu(menu: &BarMenu) -> Menu {
    Menu::new(menu.title.clone()).items(
        menu.entries
            .iter()
            .filter(|entry| HIDDEN_KEPT || !entry.hidden)
            .map(platform_item),
    )
}

/// Collects the index paths of entries in `menu` (at path `at`) and its submenus that match
/// `such`.
#[cfg(target_os = "macos")]
fn places_of(
    menu: &BarMenu,
    at: Vec<usize>,
    such: &dyn Fn(&BarEntry) -> bool,
    places: &mut Vec<Vec<usize>>,
) {
    for (index, entry) in menu.entries.iter().enumerate() {
        let mut place = at.clone();
        place.push(index);
        if let Some(submenu) = &entry.submenu {
            places_of(submenu, place.clone(), such, places);
        }
        if such(entry) {
            places.push(place);
        }
    }
}

/// Whether `entry` pastes. Paste is enabled only with text or files on the clipboard: fields
/// paste text, as AppKit text views do, and drive folders paste files.
#[cfg(target_os = "macos")]
fn pastes(entry: &BarEntry) -> bool {
    matches!(entry.role, Role::Paste | Role::PasteAndMatchStyle)
}

fn platform_item(entry: &BarEntry) -> MenuItem {
    if entry.kind == Kind::Separator {
        return MenuItem::separator();
    }
    if entry.role == Role::Services {
        // The system fills this submenu.
        return MenuItem::os_submenu(entry.title.clone(), SystemMenuType::Services);
    }
    if let Some(submenu) = &entry.submenu {
        return MenuItem::submenu(platform_menu(&BarMenu {
            title: entry.title.clone(),
            ..submenu.clone()
        }));
    }
    let title = entry.title.clone();
    if is_system(entry) {
        return match os_action(entry.role) {
            Some((action, os)) => MenuItem::Action {
                name: title.into(),
                action,
                os_action: Some(os),
                checked: false,
                disabled: false,
            },
            None => MenuItem::Action {
                name: title.into(),
                action: system_action(entry.role),
                os_action: None,
                checked: false,
                disabled: false,
            },
        };
    }
    let id = entry.id.clone();
    let checked = entry.check == Check::On;
    if entry.enabled {
        MenuItem::action(title, RunCommand { id }).checked(checked)
    } else {
        MenuItem::action(title, CommandOff { id })
            .checked(checked)
            .disabled(true)
    }
}

/// The field action for a text role, and its native `OsAction`.
fn os_action(role: Role) -> Option<(Box<dyn Action>, OsAction)> {
    Some(match role {
        Role::Undo => (Box::new(input::Undo), OsAction::Undo),
        Role::Redo => (Box::new(input::Redo), OsAction::Redo),
        Role::Cut => (Box::new(input::Cut), OsAction::Cut),
        Role::Copy => (Box::new(input::Copy), OsAction::Copy),
        Role::Paste => (Box::new(input::Paste), OsAction::Paste),
        Role::SelectAll => (Box::new(input::SelectAll), OsAction::SelectAll),
        Role::None
        | Role::About
        | Role::Services
        | Role::Hide
        | Role::HideOthers
        | Role::ShowAll
        | Role::PasteAndMatchStyle
        | Role::Delete
        | Role::StartSpeaking
        | Role::StopSpeaking
        | Role::FullScreen
        | Role::Minimize
        | Role::Zoom
        | Role::BringAllToFront => return None,
    })
}

/// The field action for a text role that has no native `OsAction`.
fn text_action(role: Role) -> Option<Box<dyn Action>> {
    Some(match role {
        Role::PasteAndMatchStyle => Box::new(input::PasteAndMatchStyle),
        Role::Delete => Box::new(input::DeleteSelection),
        Role::StartSpeaking => Box::new(input::StartSpeaking),
        Role::StopSpeaking => Box::new(input::StopSpeaking),
        Role::None
        | Role::About
        | Role::Services
        | Role::Hide
        | Role::HideOthers
        | Role::ShowAll
        | Role::Undo
        | Role::Redo
        | Role::Cut
        | Role::Copy
        | Role::Paste
        | Role::SelectAll
        | Role::FullScreen
        | Role::Minimize
        | Role::Zoom
        | Role::BringAllToFront => return None,
    })
}

/// The action for a system role without an `OsAction`: a field text command, or a command the
/// app carries out.
fn system_action(role: Role) -> Box<dyn Action> {
    text_action(role).unwrap_or_else(|| Box::new(SystemCommand { role }))
}

fn run_system(role: Role, window: &mut Window, cx: &mut App) {
    match role {
        Role::Hide => cx.hide(),
        Role::HideOthers => cx.hide_other_apps(),
        Role::ShowAll => cx.unhide_other_apps(),
        Role::Minimize => window.minimize_window(),
        Role::Zoom => window.zoom_window(),
        Role::FullScreen => window.toggle_fullscreen(),
        #[cfg(target_os = "macos")]
        Role::About => kit::platform::show_about_panel(),
        #[cfg(target_os = "macos")]
        Role::BringAllToFront => kit::platform::bring_all_to_front(),
        #[cfg(not(target_os = "macos"))]
        Role::About | Role::BringAllToFront => not_carried_out(role),
        Role::None
        | Role::Services
        | Role::Undo
        | Role::Redo
        | Role::Cut
        | Role::Copy
        | Role::Paste
        | Role::PasteAndMatchStyle
        | Role::Delete
        | Role::SelectAll
        | Role::StartSpeaking
        | Role::StopSpeaking => not_carried_out(role),
    }
}

fn not_carried_out(role: Role) {
    eprintln!("gpui\tthe system command {role:?} is not carried out by this shell yet");
}

/// Runs the menu entry for a key no view took and returns whether it handled the key. On a Mac
/// platform commands are left to the menu bar, which sends them as AppKit does; elsewhere they run
/// here.
pub fn key(
    entry: &BarEntry,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut gpui::Context<Workspace>,
) -> bool {
    if is_system(entry) {
        if cfg!(target_os = "macos") {
            return false;
        }
        match os_action(entry.role)
            .map(|(action, _)| action)
            .or_else(|| text_action(entry.role))
        {
            Some(action) => window.dispatch_action(action, cx),
            None => run_system(entry.role, window, cx),
        }
        return true;
    }
    let id = c(&entry.id);
    // SAFETY: a NUL-terminated string.
    let said = Said::of(&unsafe { nr_menu_run(NrMenu::Bar, id.as_ptr()) });
    effects::carry_out(workspace, said, window, cx);
    true
}

// MARK: Check reports

/// The menus as shown, one per line as `title: item | item`. Separators are `-`, ticked items
/// start with `✓`, and disabled items end with ` (off)`.
pub fn outline() -> String {
    menus()
        .iter()
        .enumerate()
        .map(|(index, menu)| {
            let mut items: Vec<String> = menu
                .entries
                .iter()
                .filter(|entry| !entry.hidden)
                .map(outlined)
                .collect();
            // Plus items the platform appends, such as AppKit's Writing Tools, AutoFill, Start
            // Dictation and Emoji & Symbols in the Edit menu.
            items.extend(kit::platform::added_items(index, items.len()));
            format!("{}: {}", menu.title, items.join(" | "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One entry in `outline` form.
fn outlined(entry: &BarEntry) -> String {
    if entry.kind == Kind::Separator {
        return "-".to_owned();
    }
    // System commands go to whatever has focus, so the outline always counts them enabled.
    let usable = entry.submenu.is_some() || is_system(entry) || entry.enabled;
    format!(
        "{}{}{}",
        if entry.check == Check::On { "✓" } else { "" },
        entry.title,
        if usable { "" } else { " (off)" }
    )
}

/// Every menu bar shortcut, hidden entries included, one per line as
/// `menu<TAB>title<TAB>key<TAB>held keys`.
pub fn keys() -> String {
    fn lines(menu: &BarMenu, said: &mut Vec<String>) {
        for entry in &menu.entries {
            if !entry.key.is_empty() {
                said.push(format!(
                    "{}\t{}\t{}\t{}",
                    menu.title, entry.title, entry.key, entry.held
                ));
            }
            if let Some(submenu) = &entry.submenu {
                lines(submenu, said);
            }
        }
    }
    let mut said = Vec::new();
    for menu in menus() {
        lines(&menu, &mut said);
    }
    said.join("\n")
}

/// The tray menu's current rows.
pub fn tray_outline() -> String {
    tray_menu()
        .entries
        .iter()
        .map(|entry| {
            if entry.kind == Kind::Separator {
                return "-".to_owned();
            }
            if entry.key.is_empty() {
                entry.title.clone()
            } else {
                format!("{} ⌘{}", entry.title, entry.key.to_uppercase())
            }
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

pub fn register(table: &mut crate::script::Table) {
    use crate::script::{Answer, Route};
    table.answer(Route::Kind(NrScriptKind::MenuBar), |_, _, _, _| {
        eprintln!("{}", outline());
        Answer::Done
    });
    table.answer(Route::Kind(NrScriptKind::MenuKeys), |_, _, _, _| {
        for line in keys().lines() {
            eprintln!("menukey\t{line}");
        }
        Answer::Done
    });
    table.answer(Route::Kind(NrScriptKind::TrayMenu), |_, _, _, _| {
        eprintln!("tray\t{}", tray_outline());
        Answer::Done
    });
}
