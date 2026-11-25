//! Every menu the app shows, as data: the menu bar, the tray, a folder's card, row and
//! background menus, and the list header's options. Each entry has the id the shell passes back
//! when it is chosen (`menus::ffi::nr_menu_run`), its label, glyph, shortcut, whether it is
//! enabled, ticked, hidden or destructive, and its submenu. A system command (Services, Hide,
//! Full Screen…) carries a `Role` the platform performs itself.
//!
//! Labels come from the message catalogs (`strings`) in the interface language (English,
//! ADR-0001).

pub mod ffi;
pub mod keys;
pub mod layout;
pub mod slash;

use crate::columns::View;
use crate::glyph::Glyph;
use crate::list::SortKey;
use crate::strings::{self, key, Key};
use crate::theme::Token;

/// Shortcut modifier bits, numbered as `nr_rows_key` numbers them.
pub const COMMAND: u32 = crate::ffi::NR_HELD_COMMAND;
pub const CONTROL: u32 = crate::ffi::NR_HELD_CONTROL;
pub const OPTION: u32 = crate::ffi::NR_HELD_OPTION;
pub const SHIFT: u32 = crate::ffi::NR_HELD_SHIFT;

/// The delete key in a menu shortcut, shown as "Del".
pub const DELETE_KEY: char = '\u{7f}';

/// A menu entry's shortcut: the key's character and its modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: char,
    pub held: u32,
}

impl Shortcut {
    pub const fn command(key: char) -> Self {
        Self { key, held: COMMAND }
    }

    pub const fn with(key: char, held: u32) -> Self {
        Self { key, held }
    }

    /// The shortcut as displayed: ⌘, ⌥, ⇧, ⌃, then the key in capitals (`⌘⇧.`). The delete key
    /// reads "Del".
    pub fn text(self) -> String {
        if self.key == DELETE_KEY {
            return "Del".to_owned();
        }
        let mut text = String::new();
        for (bit, glyph) in [(COMMAND, '⌘'), (OPTION, '⌥'), (SHIFT, '⇧'), (CONTROL, '⌃')] {
            if self.held & bit != 0 {
                text.push(glyph);
            }
        }
        text.extend(self.key.to_uppercase());
        text
    }
}

/// An entry's tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    /// Applies to some of the files: a dash.
    Mixed,
}

/// What kind of row an entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Does something when chosen, or opens its submenu.
    Item,
    Separator,
    /// Non-interactive text: a heading, or why the items above are disabled.
    Label,
}

/// A command the platform performs itself, with its own key handling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    None,
    About,
    /// The system's Services submenu, which the platform fills.
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    PasteAndMatchStyle,
    Delete,
    SelectAll,
    StartSpeaking,
    StopSpeaking,
    FullScreen,
    Minimize,
    Zoom,
    BringAllToFront,
}

/// A top-level menu bar menu's role for the platform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRole {
    None,
    /// The platform lists the open windows in it.
    Window,
    Help,
}

/// A dot before an entry's label in place of a glyph, showing a terminal session's activity as
/// on its card and tab (`terminal::activity::Look`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub colour: Token,
    /// Multiplies the colour's own opacity.
    pub fade: f64,
    /// The shape it takes.
    pub sign: crate::terminal::activity::Sign,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The id passed to `nr_menu_run` when chosen; empty for an inert entry.
    pub id: String,
    pub kind: Kind,
    pub title: String,
    /// The glyph before its label.
    pub icon: Option<Glyph>,
    /// A dot before its label in place of a glyph.
    pub dot: Option<Dot>,
    /// Muted text on a second line under the item's label; empty for none.
    pub detail: String,
    /// The shortcut key (empty for none), its modifiers (`COMMAND`…), and the shortcut as written
    /// beside the label.
    pub key: String,
    pub held: u32,
    pub shortcut: String,
    pub enabled: bool,
    pub check: Check,
    /// Hidden, but its shortcut still works.
    pub hidden: bool,
    /// Drawn in the warning colour.
    pub destructive: bool,
    /// Choosing it keeps the menu open and redraws it.
    pub keeps_open: bool,
    pub role: Role,
    pub submenu: Option<Menu>,
    /// The entry id run by an X at the row's far end while the row is highlighted; a session's row
    /// uses it to end the session. Empty for no X.
    pub ends: String,
}

impl Entry {
    pub fn item(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: Kind::Item,
            title: title.into(),
            icon: None,
            dot: None,
            detail: String::new(),
            key: String::new(),
            held: 0,
            shortcut: String::new(),
            enabled: true,
            check: Check::Off,
            hidden: false,
            destructive: false,
            keeps_open: false,
            role: Role::None,
            submenu: None,
            ends: String::new(),
        }
    }

    /// A command the platform performs.
    pub fn system(role: Role, title: &str) -> Self {
        Self {
            role,
            ..Self::item("", title)
        }
    }

    pub fn separator() -> Self {
        Self {
            kind: Kind::Separator,
            ..Self::item("", "")
        }
    }

    pub fn label(title: impl Into<String>) -> Self {
        Self {
            kind: Kind::Label,
            ..Self::item("", title)
        }
    }

    pub fn submenu(title: &str, menu: Menu) -> Self {
        Self {
            submenu: Some(menu),
            ..Self::item("", title)
        }
    }

    pub fn icon(mut self, icon: Glyph) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Adds an X at the row's far end, shown while highlighted, that runs entry `id`.
    pub fn ending(mut self, id: impl Into<String>) -> Self {
        self.ends = id.into();
        self
    }

    pub fn dot(mut self, dot: Dot) -> Self {
        self.dot = Some(dot);
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn keys(mut self, shortcut: Shortcut) -> Self {
        self.key = shortcut.key.to_string();
        self.held = shortcut.held;
        self.shortcut = shortcut.text();
        self
    }

    /// Shows `shortcut` beside the label without binding it, for a heading that names the keys of
    /// the rows under it.
    pub fn written_keys(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = shortcut.text();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn checked(mut self, check: Check) -> Self {
        self.check = check;
        self
    }

    pub fn ticked(self, on: bool) -> Self {
        self.checked(if on { Check::On } else { Check::Off })
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    pub fn destructive(mut self) -> Self {
        self.destructive = true;
        self
    }

    pub fn keeps_open(mut self) -> Self {
        self.keeps_open = true;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub role: MenuRole,
    pub entries: Vec<Entry>,
    /// Minimum width; 0 to fit the rows.
    pub min_width: f64,
    /// Row indices that follow the row above with no gap.
    pub tight: Vec<usize>,
    /// Maximum height, beyond which rows scroll; 0 for no limit.
    pub max_height: f64,
    /// The size of glyphs before labels, and the gap from a glyph to its label.
    pub icon_size: f64,
    pub icon_gap: f64,
    /// Labels are small muted headings over the rows under them, and shortcuts are set in
    /// monospace, as in the page's block menu.
    pub quiet_labels: bool,
}

impl Menu {
    /// The menu, this one or a submenu, that holds entry `id`.
    pub fn holding(&self, id: &str) -> Option<&Menu> {
        if self.entries.iter().any(|entry| entry.id == id) {
            return Some(self);
        }
        self.entries
            .iter()
            .filter_map(|entry| entry.submenu.as_ref())
            .find_map(|submenu| submenu.holding(id))
    }

    pub fn new(title: &str, entries: Vec<Entry>) -> Self {
        let plain = crate::menus::layout::PLAIN;
        Self {
            title: title.to_owned(),
            role: MenuRole::None,
            entries,
            min_width: plain.min_width,
            tight: Vec::new(),
            max_height: plain.max_height,
            icon_size: plain.icon_size,
            icon_gap: plain.icon_gap,
            quiet_labels: plain.quiet_labels,
        }
    }

    pub(crate) fn wide(mut self, min_width: f64) -> Self {
        self.min_width = min_width;
        self
    }

    /// The entry with `id`, here or in a submenu.
    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find_map(|entry| {
            if !entry.id.is_empty() && entry.id == id {
                return Some(entry);
            }
            entry.submenu.as_ref().and_then(|menu| menu.find(id))
        })
    }
}

// MARK: The menu bar and the tray

/// The app state the menu bar depends on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BarState<'a> {
    /// The update item's label and whether it is enabled (`update::controller::Status`).
    pub update_label: &'a str,
    pub update_enabled: bool,
    /// A folder is on screen: a drive is open and settings is not.
    pub folder_shows: bool,
    /// The drive can be changed; not in a check that names its own drive.
    pub drive_changeable: bool,
    /// How folders are shown.
    pub view: View,
    /// The interface language code for the labels.
    pub language: &'a str,
}

/// The action of a menu bar or tray item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarCommand {
    CheckForUpdates,
    ShowSettings,
    /// Opens the Changes tab of the root with the latest turn, else of the folder on screen.
    ShowChanges,
    /// Opens the system folder picker to choose another drive.
    ChooseDrive,
    ShowView(View),
    Reload,
    ToggleHiddenFiles,
    TogglePalette,
    NewTab,
    CloseTab,
    /// Reopens the most recently closed tab (`Columns::reopen_closed_tab`).
    ReopenClosedTab,
    /// Quits, confirming first while sessions are working or waiting (`quit_prompt`).
    Quit,
    Back,
    Forward,
    OpenDiagnosticLogs,
    OpenHelp,
    ShowWindow,
    /// Goes to the next session waiting for the person (`sessions::next`).
    ShowNextSession,
}

/// Where Help → Northroom Help goes.
pub const HELP_ADDRESS: &str = "https://northroom.com";

/// Each view's label and key, in `View::ALL` order. A view's menu bar id is its name.
const VIEWS: [(Key, View, char); 3] = [
    (key::MENU_VIEW_BOARD, View::Board, '1'),
    (key::MENU_VIEW_LIST, View::List, '2'),
    (key::MENU_VIEW_COLUMNS, View::Columns, '3'),
];

/// The shortcut that jumps to the next session waiting for the person.
pub const NEXT_SESSION_KEYS: Shortcut = Shortcut::with('u', COMMAND | SHIFT);
/// ⌘⇧T: Reopen Closed Tab, as in every browser.
pub const REOPEN_CLOSED_TAB_KEYS: Shortcut = Shortcut::with('t', COMMAND | SHIFT);

/// Finder's shortcut for showing hidden files, also listed in the command palette.
pub const HIDDEN_FILES_KEYS: Shortcut = Shortcut::with('.', COMMAND | SHIFT);

/// The menu bar. The view choices are enabled while a folder shows, with the current one
/// ticked. The update item shows the update's status and is disabled while one is in progress.
pub fn menu_bar(state: BarState) -> Vec<Menu> {
    let say = |key: Key| strings::text(state.language, key);
    let views = VIEWS.iter().map(|(title, view, key)| {
        Entry::item(view.name(), say(*title))
            .keys(Shortcut::command(*key))
            .enabled(state.folder_shows)
            .ticked(state.folder_shows && state.view == *view)
    });
    let speech = Menu::new(
        say(key::MENU_SPEECH),
        vec![
            Entry::system(Role::StartSpeaking, say(key::MENU_START_SPEAKING)),
            Entry::system(Role::StopSpeaking, say(key::MENU_STOP_SPEAKING)),
        ],
    );
    let mut window = Menu::new(
        say(key::MENU_WINDOW),
        vec![
            Entry::system(Role::Minimize, say(key::MENU_MINIMIZE)).keys(Shortcut::command('m')),
            Entry::system(Role::Zoom, say(key::MENU_ZOOM)),
            Entry::separator(),
            Entry::system(Role::BringAllToFront, say(key::MENU_BRING_ALL_TO_FRONT)),
        ],
    );
    window.role = MenuRole::Window;
    let mut help = Menu::new(
        say(key::MENU_HELP),
        vec![
            Entry::item("logs", say(key::MENU_OPEN_DIAGNOSTIC_LOGS)),
            Entry::separator(),
            Entry::item("help", say(key::MENU_NORTHROOM_HELP)),
        ],
    );
    help.role = MenuRole::Help;
    vec![
        Menu::new(
            "Northroom",
            vec![
                Entry::system(Role::About, say(key::MENU_ABOUT)),
                Entry::item("updates", state.update_label).enabled(state.update_enabled),
                Entry::separator(),
                Entry::item("settings", say(key::MENU_SETTINGS)).keys(Shortcut::command(',')),
                Entry::item("change-drive", say(key::MENU_CHANGE_DRIVE_FOLDER))
                    .enabled(state.drive_changeable),
                Entry::separator(),
                Entry {
                    submenu: Some(Menu::new(say(key::MENU_SERVICES), Vec::new())),
                    ..Entry::system(Role::Services, say(key::MENU_SERVICES))
                },
                Entry::separator(),
                Entry::system(Role::Hide, say(key::MENU_HIDE)).keys(Shortcut::command('h')),
                Entry::system(Role::HideOthers, say(key::MENU_HIDE_OTHERS))
                    .keys(Shortcut::with('h', COMMAND | OPTION)),
                Entry::system(Role::ShowAll, say(key::MENU_SHOW_ALL)),
                Entry::separator(),
                Entry::item("quit", say(key::MENU_QUIT)).keys(Shortcut::command('q')),
            ],
        ),
        Menu::new(
            say(key::MENU_EDIT),
            vec![
                Entry::system(Role::Undo, say(key::MENU_UNDO)).keys(Shortcut::command('z')),
                Entry::system(Role::Redo, say(key::MENU_REDO))
                    .keys(Shortcut::with('z', COMMAND | SHIFT)),
                Entry::separator(),
                Entry::system(Role::Cut, say(key::MENU_CUT)).keys(Shortcut::command('x')),
                Entry::system(Role::Copy, say(key::MENU_COPY)).keys(Shortcut::command('c')),
                Entry::system(Role::Paste, say(key::MENU_PASTE)).keys(Shortcut::command('v')),
                Entry::system(
                    Role::PasteAndMatchStyle,
                    say(key::MENU_PASTE_AND_MATCH_STYLE),
                )
                .keys(Shortcut::with('v', COMMAND | OPTION | SHIFT)),
                Entry::system(Role::Delete, say(key::MENU_DELETE)),
                Entry::system(Role::SelectAll, say(key::MENU_SELECT_ALL))
                    .keys(Shortcut::command('a')),
                Entry::separator(),
                Entry::submenu(say(key::MENU_SPEECH), speech),
            ],
        ),
        Menu::new(
            say(key::MENU_VIEW),
            views
                .chain([
                    Entry::separator(),
                    Entry::item("changes", say(key::MENU_SHOW_CHANGES)).enabled(state.folder_shows),
                    Entry::item("reload", say(key::MENU_RELOAD)).keys(Shortcut::command('r')),
                    Entry::system(Role::FullScreen, say(key::MENU_TOGGLE_FULL_SCREEN))
                        .keys(Shortcut::with('f', COMMAND | CONTROL)),
                    Entry::item("hidden-files", say(key::MENU_HIDDEN_FILES))
                        .keys(HIDDEN_FILES_KEYS)
                        .hidden(),
                    Entry::item("palette", say(key::MENU_COMMAND_PALETTE))
                        .keys(Shortcut::command('k'))
                        .hidden(),
                    Entry::item("new-tab", say(key::MENU_NEW_TAB))
                        .keys(Shortcut::command('t'))
                        .hidden(),
                    Entry::item("close-tab", say(key::MENU_CLOSE_TAB))
                        .keys(Shortcut::command('w'))
                        .hidden(),
                    Entry::item("reopen-closed-tab", say(key::MENU_REOPEN_CLOSED_TAB))
                        .keys(REOPEN_CLOSED_TAB_KEYS)
                        .hidden(),
                    Entry::item("back", say(key::MENU_BACK))
                        .keys(Shortcut::command('['))
                        .hidden(),
                    Entry::item("forward", say(key::MENU_FORWARD))
                        .keys(Shortcut::command(']'))
                        .hidden(),
                    Entry::item("next-session", say(key::MENU_NEXT_SESSION))
                        .keys(NEXT_SESSION_KEYS)
                        .hidden(),
                ])
                .collect(),
        ),
        window,
        help,
    ]
}

/// The menu of the menu bar's Northroom icon: each session in `running` waiting for the person
/// (`sessions::waiting`), by name and request, which opens it; then Open Northroom, the update
/// item as in the menu bar, and Quit Northroom.
pub fn tray(
    update_label: &str,
    update_enabled: bool,
    running: &[crate::sessions::Running],
    language: &str,
) -> Menu {
    let say = |key: Key| strings::text(language, key);
    let mut entries: Vec<Entry> = crate::sessions::waiting(running)
        .into_iter()
        .map(|session| {
            Entry::item(
                session_entry_of(&session.id),
                format!("{} · {}", session.name, session.activity.label(language)),
            )
        })
        .collect();
    if !entries.is_empty() {
        entries.push(Entry::separator());
    }
    entries.extend([
        Entry::item("open", say(key::MENU_OPEN_NORTHROOM)),
        Entry::separator(),
        Entry::item("updates", update_label).enabled(update_enabled),
        Entry::item("quit", say(key::MENU_QUIT)).keys(Shortcut::command('q')),
    ]);
    Menu::new("", entries)
}

/// The tooltip on the menu bar's Northroom icon.
pub const TRAY_TOOLTIP: &str = "Northroom";

/// The action for a menu bar or tray item id.
pub fn bar_command(id: &str) -> Option<BarCommand> {
    Some(match id {
        "updates" => BarCommand::CheckForUpdates,
        "settings" => BarCommand::ShowSettings,
        "changes" => BarCommand::ShowChanges,
        "change-drive" => BarCommand::ChooseDrive,
        "board" => BarCommand::ShowView(View::Board),
        "list" => BarCommand::ShowView(View::List),
        "columns" => BarCommand::ShowView(View::Columns),
        "reload" => BarCommand::Reload,
        "hidden-files" => BarCommand::ToggleHiddenFiles,
        "palette" => BarCommand::TogglePalette,
        "new-tab" => BarCommand::NewTab,
        "close-tab" => BarCommand::CloseTab,
        "quit" => BarCommand::Quit,
        "reopen-closed-tab" => BarCommand::ReopenClosedTab,
        "back" => BarCommand::Back,
        "forward" => BarCommand::Forward,
        "logs" => BarCommand::OpenDiagnosticLogs,
        "help" => BarCommand::OpenHelp,
        "open" => BarCommand::ShowWindow,
        "next-session" => BarCommand::ShowNextSession,
        _ => return None,
    })
}

/// The action of a palette command (`commands::drive_commands`), the same as its menu item's.
pub fn palette_command(id: &str) -> Option<BarCommand> {
    match id {
        "drive.toggle-hidden-files" => Some(BarCommand::ToggleHiddenFiles),
        "drive.change-folder" => Some(BarCommand::ChooseDrive),
        _ => None,
    }
}

// MARK: A folder's menus

/// The desktop platform, for folder menu entries that differ by platform: the file manager's
/// name, and Linux has no share sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Mac,
    Windows,
    Linux,
}

impl Desktop {
    /// The desktop this app is built for.
    pub const fn this() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// Whether it has a file share sheet: macOS and Windows do, Linux does not.
    fn shares(self) -> bool {
        match self {
            Self::Mac | Self::Windows => true,
            Self::Linux => false,
        }
    }

    /// "Show in Finder", "Show in File Explorer", or on Linux "Show in Folder".
    fn reveal_words(self) -> Key {
        match self {
            Self::Mac => key::MENU_SHOW_IN_FINDER,
            Self::Windows => key::MENU_SHOW_IN_FILE_EXPLORER,
            Self::Linux => key::MENU_SHOW_IN_FOLDER,
        }
    }
}

/// The clipboard contents, as a folder's Paste sees them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clipboard {
    Files,
    Other,
}

/// Whether a file's folder accepts new files beside it, as Compress creates. A system-protected
/// folder does not, just as it takes no import or paste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beside {
    TakesFiles,
    Protected,
}

/// One of the files a folder's menu is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    pub id: String,
    pub title: String,
    /// Why the system protects it from changes, as shown text; `None` for an ordinary file.
    pub protection: Option<&'static str>,
    /// Whether its folder accepts new files. A menu's archive goes beside its first file.
    pub beside: Beside,
    /// A terminal file; several opened together share one tab, side by side.
    pub terminal: bool,
    /// A terminal file with a running session, which its menu can end.
    pub running: bool,
    /// A folder, whose changes its menu can show.
    pub folder: bool,
}

/// View ▸ as Board, as List, as Columns, at the end of every drive menu. The three choices form
/// one 144 pt group with no gap between rows.
fn view_entries(shown: View, language: &str) -> [Entry; 2] {
    let mut choices = Menu::new(
        "",
        VIEWS
            .iter()
            .map(|(title, view, key)| {
                Entry::item(
                    format!("view:{}", view.number()),
                    strings::text(language, *title),
                )
                .keys(Shortcut::command(*key))
                .ticked(shown == *view)
            })
            .collect(),
    )
    .wide(144.0);
    choices.tight = vec![1, 2];
    [
        Entry::separator(),
        Entry::submenu(strings::text(language, key::MENU_VIEW), choices).icon(Glyph::SquaresFour),
    ]
}

fn trash_entry(enabled: bool, language: &str) -> Entry {
    Entry::item("trash", strings::text(language, key::MENU_MOVE_TO_TRASH))
        .icon(Glyph::Trash)
        .enabled(enabled)
        .destructive()
}

/// The menu for a board card or a list or column row, about `group` (one card, or the rows from
/// `menu_group`). It follows Finder's grouping and order: Open, Open in New Tab, Move to Trash,
/// Rename (for one file), Compress and Duplicate, Copy, Share… and Show in Finder, then why the
/// first protected file is protected, and the view. Like Finder's, it names neither the file nor
/// the count and shows no shortcuts. A protected file cannot be renamed or trashed; read-only
/// actions stay enabled. Compress writes its archive beside the first file, so it is enabled
/// only where that folder accepts new files.
pub fn file_menu(group: &[File], view: View, desktop: Desktop, language: &str) -> Menu {
    let say = |key: Key| strings::text(language, key);
    let protection = group.iter().find_map(|file| file.protection);
    let free = protection.is_none();
    let one = match group {
        [file] => Some(file),
        _ => None,
    };
    let opening = opening_entries(group, language);
    let mut changing = Vec::new();
    if one.is_some() {
        changing.push(
            Entry::item("rename", say(key::MENU_RENAME))
                .icon(Glyph::PencilSimple)
                .enabled(free),
        );
    }
    let beside = group.first().map(|file| file.beside);
    changing.extend([
        Entry::item("compress", say(key::MENU_COMPRESS))
            .icon(Glyph::FileZip)
            .enabled(beside == Some(Beside::TakesFiles)),
        Entry::item("duplicate", say(key::MENU_DUPLICATE)).icon(Glyph::Copy),
    ]);
    let mut sending = vec![Entry::item("copy", say(key::MENU_COPY)).icon(Glyph::Clipboard)];
    if desktop.shares() {
        sending.push(Entry::item("share", say(key::MENU_SHARE)).icon(Glyph::Export));
    }
    sending.push(Entry::item("reveal", say(desktop.reveal_words())).icon(Glyph::MagnifyingGlass));
    let labelling: Vec<Entry> = protection.map(Entry::label).into_iter().collect();
    let groups = [
        opening,
        vec![trash_entry(free, language)],
        changing,
        sending,
        labelling,
    ];
    let mut entries: Vec<Entry> = Vec::new();
    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        if !entries.is_empty() {
            entries.push(Entry::separator());
        }
        entries.extend(group);
    }
    entries.extend(view_entries(view, language));
    Menu::new("", entries).wide(192.0)
}

/// Open and Open in New Tab for one file, with End Session for a running terminal and Show
/// Changes for a folder; Open alone for terminals selected together, which open side by side
/// in one tab.
fn opening_entries(group: &[File], language: &str) -> Vec<Entry> {
    let say = |key: Key| strings::text(language, key);
    let open = || Entry::item("open", say(key::MENU_OPEN)).icon(Glyph::FolderOpen);
    let [file] = group else {
        let together = group.len() > 1 && group.iter().all(|file| file.terminal);
        return if together { vec![open()] } else { Vec::new() };
    };
    let mut opening = vec![
        open(),
        Entry::item("open-new-tab", say(key::MENU_OPEN_IN_NEW_TAB)).icon(Glyph::Plus),
    ];
    if file.running {
        opening.push(Entry::item("end-session", say(key::MENU_END_SESSION)).icon(Glyph::Close));
    }
    if file.folder {
        opening.push(show_changes_entry(language));
    }
    opening
}

/// Show Changes, which opens the Changes tab of a folder's root.
fn show_changes_entry(language: &str) -> Entry {
    Entry::item(
        "show-changes",
        strings::text(language, key::MENU_SHOW_CHANGES),
    )
    .icon(Glyph::Diff)
}

/// The files a row's menu is about: the selected group when the clicked row is part of it,
/// else the clicked row alone.
pub fn menu_group<'a>(clicked: &'a str, selected: &'a [String]) -> Vec<&'a str> {
    if selected.len() > 1 && selected.iter().any(|id| id == clicked) {
        selected.iter().map(String::as_str).collect()
    } else {
        vec![clicked]
    }
}

/// The menu for a folder's empty space, 176 pt wide: what can be created in it, import and
/// paste, the view, and the sort order (`sorted_by`, shared by the board and the list). Paste is
/// enabled while the clipboard holds files. In a system-protected folder (`refusal`) the new
/// terminal, import and paste entries are omitted, and New page and New folder are disabled.
pub fn background_menu(
    refusal: Option<&str>,
    clipboard: Clipboard,
    view: View,
    sorted_by: Option<SortKey>,
    language: &str,
) -> Menu {
    let say = |key: Key| strings::text(language, key);
    let open = refusal.is_none();
    let mut entries = Vec::new();
    entries.push(
        Entry::item("new-page", say(key::MENU_NEW_PAGE))
            .icon(Glyph::FilePlus)
            .enabled(open),
    );
    entries.push(
        Entry::item("new-folder", say(key::MENU_NEW_FOLDER))
            .icon(Glyph::FolderPlus)
            .enabled(open),
    );
    if open {
        entries.extend([
            Entry::item("new-terminal", say(key::MENU_NEW_TERMINAL)).icon(Glyph::TerminalWindow),
            Entry::separator(),
            Entry::item("import", say(key::MENU_IMPORT)).icon(Glyph::UploadSimple),
            Entry::item("paste", say(key::MENU_PASTE))
                .icon(Glyph::Clipboard)
                .enabled(clipboard == Clipboard::Files),
        ]);
    }
    entries.push(Entry::separator());
    entries.push(show_changes_entry(language));
    entries.extend(view_entries(view, language));
    entries.push(sort_entry(sorted_by, language));
    Menu::new("", entries).wide(176.0)
}

/// The sort keys a folder's menu offers, as in Finder's Sort By.
pub const SORT_CHOICES: [SortKey; 6] = [
    SortKey::Name,
    SortKey::Kind,
    SortKey::Modified,
    SortKey::Created,
    SortKey::Added,
    SortKey::Size,
];

/// Sort By ▸ Name, Kind, the dates and Size, with the folder's sort ticked, or none ticked while
/// a list column sorts by something else.
fn sort_entry(sorted_by: Option<SortKey>, language: &str) -> Entry {
    let choices = SORT_CHOICES
        .iter()
        .map(|key| {
            Entry::item(format!("sort:{}", key.number()), key.label())
                .ticked(sorted_by == Some(*key))
        })
        .collect();
    Entry::submenu(
        strings::text(language, key::MENU_SORT_BY),
        Menu::new("", choices).wide(144.0),
    )
    .icon(Glyph::ListNumbers)
}

/// The action of a folder menu entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderCommand {
    Open,
    /// Opens the file in a new tab beside the active one.
    OpenInNewTab,
    /// Ends the terminal file's session, keeping the file.
    EndSession,
    Rename,
    /// Copies each file beside itself with a number.
    Duplicate,
    /// Writes the files into one zip archive beside the first.
    Compress,
    /// Copies the files to the clipboard.
    Copy,
    /// Opens the share sheet for the files.
    Share,
    /// Shows the files in the system file manager.
    Reveal,
    Trash,
    ShowView(View),
    /// Sorts the folder by the list column with this number (`list::Column::number`).
    SortBy(u32),
    NewPage,
    NewFolder,
    NewTerminal,
    Import,
    /// Copies the clipboard's files into the folder.
    Paste,
    /// Opens the Changes tab of the folder clicked, or of the folder of the menu.
    ShowChanges,
}

/// The action for folder menu entry `id`.
pub fn folder_command(id: &str) -> Option<FolderCommand> {
    if let Some((head, rest)) = id.split_once(':') {
        return match head {
            // A number past the last view is rejected instead of mapping to another view.
            "view" => rest
                .parse()
                .ok()
                .and_then(View::numbered)
                .map(FolderCommand::ShowView),
            "sort" => rest.parse().ok().map(FolderCommand::SortBy),
            _ => None,
        };
    }
    Some(match id {
        "open" => FolderCommand::Open,
        "open-new-tab" => FolderCommand::OpenInNewTab,
        "end-session" => FolderCommand::EndSession,
        "rename" => FolderCommand::Rename,
        "duplicate" => FolderCommand::Duplicate,
        "compress" => FolderCommand::Compress,
        "copy" => FolderCommand::Copy,
        "share" => FolderCommand::Share,
        "reveal" => FolderCommand::Reveal,
        "trash" => FolderCommand::Trash,
        "new-page" => FolderCommand::NewPage,
        "new-folder" => FolderCommand::NewFolder,
        "new-terminal" => FolderCommand::NewTerminal,
        "import" => FolderCommand::Import,
        "paste" => FolderCommand::Paste,
        "show-changes" => FolderCommand::ShowChanges,
        _ => return None,
    })
}

// MARK: Dialogs a menu opens

/// A confirmation dialog: its title, the sentence under it, the field and its initial value,
/// and its two buttons. A `destructive` one has its action in the warning colour and no close
/// button.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub title: String,
    pub message: Option<String>,
    pub value: Option<String>,
    pub action: &'static str,
    pub cancel: &'static str,
    pub destructive: bool,
    /// The field holds a password: shown as bullets, never trimmed.
    pub secret: bool,
}

/// The open panel's button that copies files into the folder, in `language`.
pub fn import_button(language: &str) -> &'static str {
    strings::text(language, key::DIALOG_IMPORT)
}

fn naming(title: &str, value: &str, language: &str) -> Prompt {
    Prompt {
        title: title.to_owned(),
        message: None,
        value: Some(value.to_owned()),
        action: strings::text(language, key::COMMON_SAVE),
        cancel: strings::text(language, key::ACCOUNT_CANCEL),
        destructive: false,
        secret: false,
    }
}

/// The dialog that asks for a locked file's password, saying so when `wrong`, the last one
/// typed, didn't open it.
pub fn unlock_prompt(name: &str, wrong: bool, language: &str) -> Prompt {
    let message = if wrong {
        key::DIALOG_UNLOCK_WRONG
    } else {
        key::DIALOG_UNLOCK_MESSAGE
    };
    Prompt {
        title: strings::text(language, key::DIALOG_UNLOCK_TITLE).replace("{name}", name),
        message: Some(strings::text(language, message).to_owned()),
        value: Some(String::new()),
        action: strings::text(language, key::MENU_OPEN),
        cancel: strings::text(language, key::ACCOUNT_CANCEL),
        destructive: false,
        secret: true,
    }
}

/// The rename dialog, with the file's current name to type over.
pub fn rename_prompt(title: &str, language: &str) -> Prompt {
    naming(strings::text(language, key::MENU_RENAME), title, language)
}

/// A name from a dialog field, trimmed of surrounding whitespace; `None` when that leaves
/// nothing, since a name is required.
pub fn name_of(typed: &str) -> Option<String> {
    let name = typed.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

// MARK: Notices

/// The notice when the drive refused a change; the refusal's text goes under it.
pub fn update_failed(language: &str) -> &'static str {
    strings::text(language, key::DIALOG_UPDATE_FAILED)
}

/// The notice when files dropped on a terminal could not be passed to its program.
pub fn add_failed(language: &str) -> &'static str {
    strings::text(language, key::DIALOG_ADD_FAILED)
}

pub fn add_failed_detail(terminal: &str, language: &str) -> String {
    strings::format(
        language,
        key::DIALOG_ADD_FAILED_DETAIL,
        &[("terminal", terminal)],
    )
}

// MARK: The list header's options

/// The header column a right click targets, for its Move items: its key, its position among the
/// shown columns after Name, and how many there are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moving {
    pub key: u32,
    pub at: usize,
    pub count: usize,
}

/// A column option: its key, its name, and whether the list shows it.
pub type Choice<'a> = (u32, &'a str, bool);

/// The list header's options, capped at 28rem with the rest scrolling, after the column's own
/// Move items when a column is targeted: the columns to show (Name always, not removable),
/// folders on top, the Board's order (enabled while a column sorts), and the default view.
/// Ticks toggle in place and the menu stays open.
pub fn list_options(
    moving: Option<Moving>,
    choices: &[Choice],
    folders_first: bool,
    language: &str,
) -> Menu {
    let say = |key: Key| strings::text(language, key);
    let mut entries = Vec::new();
    if let Some(moving) = moving {
        for (title, step) in [
            (key::MENU_MOVE_COLUMN_LEFT, -1i64),
            (key::MENU_MOVE_COLUMN_RIGHT, 1),
        ] {
            let target = moving.at as i64 + step;
            let enabled = (0..moving.count as i64).contains(&target);
            entries.push(Entry::item(format!("move:{}", target + 1), say(title)).enabled(enabled));
        }
        entries.push(Entry::separator());
    }
    entries.push(Entry::label(say(key::MENU_SHOW_COLUMNS)));
    entries.extend(choices.iter().map(|(key, title, shown)| {
        Entry::item(format!("column:{key}"), *title)
            .ticked(*key == 0 || *shown)
            .enabled(*key != 0)
            .keeps_open()
    }));
    entries.extend([
        Entry::separator(),
        Entry::item("folders-first", say(key::MENU_FOLDERS_ON_TOP))
            .ticked(folders_first)
            .keeps_open(),
        Entry::separator(),
        Entry::item("restore", say(key::MENU_RESTORE_VIEW)),
    ]);
    let mut menu = Menu::new("", entries);
    menu.max_height = 448.0;
    menu
}

/// The action of a list header option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListCommand {
    /// Moves the targeted column to position `to` (1 is the first after Name).
    MoveColumn(usize),
    ToggleColumn(u32),
    ToggleFoldersFirst,
    Restore,
}

pub fn list_command(id: &str) -> Option<ListCommand> {
    if let Some((head, rest)) = id.split_once(':') {
        return match head {
            "move" => rest.parse().ok().map(ListCommand::MoveColumn),
            "column" => rest.parse().ok().map(ListCommand::ToggleColumn),
            _ => None,
        };
    }
    Some(match id {
        "folders-first" => ListCommand::ToggleFoldersFirst,
        "restore" => ListCommand::Restore,
        _ => return None,
    })
}

// MARK: The page's menus

/// The menu for a page block of `kind`, opened from its ⋮⋮ handle: duplicate, delete, and for a
/// text block the kinds it can become (dividers and tables are inserted, not converted to). It is
/// 224 pt wide, with glyphs 8 pt from their labels, a small muted "Turn into" heading and
/// monospace shortcuts.
pub fn page_block_menu(kind: &crate::page::BlockKind, language: &str) -> Menu {
    use crate::page::edit::{Kind, COMMANDS};
    use crate::page::BlockKind;
    let say = |key: Key| strings::text(language, key);
    let mut entries = vec![
        Entry::item("duplicate", say(key::MENU_DUPLICATE))
            .icon(Glyph::Copy)
            .keys(Shortcut::command('d')),
        Entry::item("delete", say(key::MENU_DELETE))
            .icon(Glyph::Trash)
            .keys(Shortcut::with(DELETE_KEY, 0)),
    ];
    let words = !matches!(
        kind,
        BlockKind::Divider
            | BlockKind::Table { .. }
            | BlockKind::Media { .. }
            | BlockKind::Player { .. }
    );
    if words {
        entries.push(Entry::label(say(key::MENU_TURN_INTO)));
        for (index, command) in COMMANDS.iter().enumerate() {
            if !matches!(command.kind, Kind::Divider | Kind::Table) {
                entries.push(
                    Entry::item(format!("turn:{index}"), say(command.label)).icon(command.icon),
                );
            }
        }
    }
    Menu {
        icon_gap: 8.0,
        quiet_labels: true,
        ..Menu::new("", entries).wide(224.0)
    }
}

/// The action of a block menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageBlockCommand {
    Duplicate,
    Delete,
    /// Converts the block to `/` menu entry `n` (`page::edit::COMMANDS`).
    Turn(usize),
}

pub fn page_block_command(id: &str) -> Option<PageBlockCommand> {
    if let Some(index) = id.strip_prefix("turn:") {
        return index
            .parse()
            .ok()
            .filter(|&index| index < crate::page::edit::COMMANDS.len())
            .map(PageBlockCommand::Turn);
    }
    Some(match id {
        "duplicate" => PageBlockCommand::Duplicate,
        "delete" => PageBlockCommand::Delete,
        _ => return None,
    })
}

// MARK: The sessions' menu

/// The id prefix of a sessions menu row, before its terminal file's id.
const SESSION_ENTRY: &str = "session:";
const END_SESSION_ENTRY: &str = "end-session:";

/// The sessions menu's maximum height, beyond which its rows scroll.
const SESSIONS_MENU_MAX_HEIGHT: f64 = 560.0;

/// The menu of the title bar's sessions button: every running session under its group heading
/// (`sessions::grouped`), each heading showing a count. Waiting sessions come first, so while
/// one waits the first heading shows the next-session shortcut, which works while the menu is
/// open. With no session running it says so. At least 300 pt wide. Review changes is the title
/// bar's own button beside it (`title_bar::Control::Review`), not a row here.
pub fn sessions_menu(running: &[crate::sessions::Running], language: &str) -> Menu {
    use crate::sessions::{grouped, next};
    let mut entries = Vec::new();
    for (group, listed) in grouped(running) {
        let heading = Entry::label(format!("{} · {}", group.label(language), listed.len()));
        let shows_keys = entries.is_empty() && next(running, None).is_some();
        entries.push(if shows_keys {
            heading.written_keys(NEXT_SESSION_KEYS)
        } else {
            heading
        });
        entries.extend(
            listed
                .into_iter()
                .map(|session| session_row(session, group)),
        );
    }
    if entries.is_empty() {
        entries.push(Entry::label(strings::text(language, key::SESSIONS_NONE)));
    }
    entries.push(
        Entry::item(
            "next-session",
            strings::text(language, key::MENU_NEXT_SESSION),
        )
        .keys(NEXT_SESSION_KEYS)
        .hidden(),
    );
    Menu {
        max_height: SESSIONS_MENU_MAX_HEIGHT,
        quiet_labels: true,
        ..Menu::new("", entries).wide(300.0)
    }
}

/// A session's row under `group`'s heading, with the group's dot. A session that needs the
/// person or is working shows its folder and activity on a second line; an idle one is a single
/// line with its folder after its name.
fn session_row(
    session: &crate::sessions::Running,
    group: crate::terminal::activity::Activity,
) -> Entry {
    use crate::terminal::activity::{look, Activity};
    let id = session_entry_of(&session.id);
    let row = match group {
        Activity::Idle => Entry::item(
            id,
            format!(
                "{} · {}",
                crate::sessions::heading_of(session),
                session.folder
            ),
        ),
        Activity::NeedsYou
        | Activity::Finished
        | Activity::Working
        | Activity::Starting
        | Activity::Ended => Entry::item(id, crate::sessions::heading_of(session))
            .detail(crate::sessions::detail_of(session)),
    };
    let row = row.ending(end_session_entry_of(&session.id));
    let shown = look(Some(group));
    match shown.dot {
        Some(colour) => row.dot(Dot {
            colour,
            fade: shown.dot_fade,
            sign: shown.sign,
        }),
        None => row,
    }
}

/// The id of the entry that shows terminal file `file`'s session.
pub fn session_entry_of(file: &str) -> String {
    format!("{SESSION_ENTRY}{file}")
}

/// The id of a session row's X, which ends terminal file `file`'s session.
pub fn end_session_entry_of(file: &str) -> String {
    format!("{END_SESSION_ENTRY}{file}")
}

/// The terminal file whose session X entry `id` ends.
pub fn end_session_entry(id: &str) -> Option<&str> {
    id.strip_prefix(END_SESSION_ENTRY)
}

/// The confirmation before quitting while `count` sessions are working or waiting, since
/// quitting ends them. `None` for zero, and the app quits at once.
pub fn quit_prompt(count: usize, language: &str) -> Option<Prompt> {
    let message = match count {
        0 => return None,
        1 => strings::text(language, key::DIALOG_QUIT_ONE).to_owned(),
        _ => strings::text(language, key::DIALOG_QUIT_MANY).replace("{count}", &count.to_string()),
    };
    Some(Prompt {
        title: strings::text(language, key::DIALOG_QUIT_TITLE).to_owned(),
        message: Some(message),
        value: None,
        action: strings::text(language, key::MENU_QUIT),
        cancel: strings::text(language, key::ACCOUNT_CANCEL),
        destructive: true,
        secret: false,
    })
}

/// The confirmation before ending `name`'s session doing `activity`: a warning while it is
/// working or waiting, like quitting a busy terminal. `None` when nothing would be lost, and it
/// ends at once.
pub fn end_session_prompt(
    name: &str,
    activity: crate::terminal::activity::Activity,
    language: &str,
) -> Option<Prompt> {
    use crate::terminal::activity::Activity;
    let message = match activity {
        Activity::Working => key::DIALOG_END_SESSION_WORKING,
        Activity::NeedsYou => key::DIALOG_END_SESSION_WAITING,
        _ => return None,
    };
    Some(Prompt {
        title: strings::text(language, key::DIALOG_END_SESSION_TITLE).replace("{name}", name),
        message: Some(strings::text(language, message).to_owned()),
        value: None,
        action: strings::text(language, key::MENU_END_SESSION),
        cancel: strings::text(language, key::ACCOUNT_CANCEL),
        destructive: true,
        secret: false,
    })
}

/// The terminal file for sessions menu row `id`.
pub fn session_entry(id: &str) -> Option<&str> {
    id.strip_prefix(SESSION_ENTRY)
}

// MARK: A tab's menu

/// The entry of a tab's menu that closes it.
pub const CLOSE_TAB_ENTRY: &str = "close-tab";
/// The tab menu entry that reopens the most recently closed tab.
pub const REOPEN_CLOSED_TAB_ENTRY: &str = "reopen-closed-tab";

/// A tab's context menu: Close tab, disabled on the last tab, and Reopen closed tab with its
/// shortcut, enabled while a closed tab is remembered. At least 160 pt wide.
pub fn tab_menu(closable: bool, reopenable: bool, language: &str) -> Menu {
    Menu::new(
        "",
        vec![
            Entry::item(
                CLOSE_TAB_ENTRY,
                strings::text(language, key::TITLE_BAR_CLOSE_TAB),
            )
            .enabled(closable),
            Entry::item(
                REOPEN_CLOSED_TAB_ENTRY,
                strings::text(language, key::TITLE_BAR_REOPEN_CLOSED_TAB),
            )
            .keys(REOPEN_CLOSED_TAB_KEYS)
            .enabled(reopenable),
        ],
    )
    .wide(160.0)
}

/// The context menu of a table cell in `row` of a `columns`-wide table: add or remove rows and
/// columns. The header row and a table's last column cannot be removed.
pub fn page_table_menu(row: usize, columns: usize, language: &str) -> Menu {
    let say = |key: Key| strings::text(language, key);
    Menu::new(
        "",
        vec![
            Entry::item("add-row", say(key::MENU_ADD_ROW_BELOW)),
            Entry::item("add-column", say(key::MENU_ADD_COLUMN)),
            Entry::separator(),
            Entry::item("delete-row", say(key::MENU_DELETE_ROW))
                .enabled(row > 0)
                .destructive(),
            Entry::item("delete-column", say(key::MENU_DELETE_COLUMN))
                .enabled(columns > 1)
                .destructive(),
        ],
    )
}

/// The action of a table cell menu entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageTableCommand {
    AddRow,
    AddColumn,
    DeleteRow,
    DeleteColumn,
}

pub fn page_table_command(id: &str) -> Option<PageTableCommand> {
    Some(match id {
        "add-row" => PageTableCommand::AddRow,
        "add-column" => PageTableCommand::AddColumn,
        "delete-row" => PageTableCommand::DeleteRow,
        "delete-column" => PageTableCommand::DeleteColumn,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(menu: &Menu) -> Vec<String> {
        menu.entries
            .iter()
            .filter(|entry| !entry.hidden)
            .map(|entry| match entry.kind {
                Kind::Separator => "-".to_owned(),
                _ => entry.title.clone(),
            })
            .collect()
    }

    #[test]
    fn a_locked_file_asks_for_its_password_in_a_secret_field() {
        let first = unlock_prompt("Report.pdf", false, "en");
        assert_eq!(first.title, "“Report.pdf” is locked");
        assert_eq!(
            first.message.as_deref(),
            Some("Type its password to open it.")
        );
        assert_eq!(
            (first.value.as_deref(), first.secret, first.action),
            (Some(""), true, "Open")
        );
        let again = unlock_prompt("Report.pdf", true, "en");
        assert_eq!(
            again.message.as_deref(),
            Some("That password didn’t open it. Try again.")
        );
    }

    #[test]
    fn terminals_selected_together_are_offered_open() {
        let terminal = |id: &str| File {
            terminal: true,
            ..file(id, None)
        };
        let together = file_menu(
            &[terminal("a"), terminal("b")],
            View::List,
            Desktop::Mac,
            "en",
        );
        assert!(together.find("open").is_some());
        let mixed = file_menu(
            &[terminal("a"), file("b", None)],
            View::List,
            Desktop::Mac,
            "en",
        );
        assert!(mixed.find("open").is_none());
    }

    fn file(id: &str, protection: Option<&'static str>) -> File {
        File {
            id: id.to_owned(),
            title: format!("{id}.txt"),
            protection,
            beside: Beside::TakesFiles,
            terminal: false,
            running: false,
            folder: false,
        }
    }

    /// Whether no separator leads, trails or doubles up, in the menu or any submenu.
    fn separators_are_tidy(menu: &Menu) -> bool {
        let separator = |entry: &Entry| entry.kind == Kind::Separator;
        let tidy = !menu.entries.first().is_some_and(separator)
            && !menu.entries.last().is_some_and(separator)
            && !menu
                .entries
                .windows(2)
                .any(|pair| separator(&pair[0]) && separator(&pair[1]));
        tidy && menu
            .entries
            .iter()
            .filter_map(|entry| entry.submenu.as_ref())
            .all(separators_are_tidy)
    }

    const STATE: BarState = BarState {
        update_label: "Check for Updates…",
        update_enabled: true,
        folder_shows: true,
        drive_changeable: true,
        view: View::List,
        language: "en",
    };

    #[test]
    fn shortcuts_read_command_option_shift_control_then_the_key() {
        assert_eq!(HIDDEN_FILES_KEYS.text(), "⌘⇧.");
        assert_eq!(Shortcut::command('1').text(), "⌘1");
        assert_eq!(Shortcut::with('v', COMMAND | OPTION | SHIFT).text(), "⌘⌥⇧V");
        assert_eq!(Shortcut::with('f', COMMAND | CONTROL).text(), "⌘⌃F");
        assert_eq!(Shortcut::with(DELETE_KEY, 0).text(), "Del");
        assert_eq!(Shortcut::command('d').text(), "⌘D");
    }

    #[test]
    fn the_menu_bar_lists_every_item_in_order() {
        let bar = menu_bar(STATE);
        let shown: Vec<String> = bar
            .iter()
            .map(|menu| format!("{}: {}", menu.title, titles(menu).join(" | ")))
            .collect();
        assert_eq!(
            shown,
            [
                "Northroom: About Northroom | Check for Updates… | - | Settings… | Change Drive Folder… | - | Services | - | Hide Northroom | Hide Others | Show All | - | Quit Northroom",
                "Edit: Undo | Redo | - | Cut | Copy | Paste | Paste and Match Style | Delete | Select All | - | Speech",
                "View: as Board | as List | as Columns | - | Show Changes | Reload | Toggle Full Screen",
                "Window: Minimize | Zoom | - | Bring All to Front",
                "Help: Open Diagnostic Logs | - | Northroom Help",
            ]
        );
        assert_eq!(
            (bar[3].role, bar[4].role),
            (MenuRole::Window, MenuRole::Help)
        );
        assert_eq!(bar[0].entries[6].role, Role::Services);
    }

    #[test]
    fn the_sessions_menu_lists_each_session_under_what_it_asks() {
        use crate::sessions::Running;
        use crate::terminal::activity::Activity;
        let session = |id: &str, folder: &str, name: &str, doing: &str, activity| Running {
            id: id.to_owned(),
            name: name.to_owned(),
            folder: folder.to_owned(),
            doing: doing.to_owned(),
            agent: String::new(),
            activity,
        };
        let running = [
            session(
                "b",
                "Finance",
                "Bookkeeping",
                "Reconciling October",
                Activity::Working,
            ),
            session("o", "Sales / Q4 leads", "Outreach", "", Activity::NeedsYou),
            session("s", "Drive", "Shell", "", Activity::Idle),
        ];
        let menu = sessions_menu(&running, "en");
        assert_eq!(
            titles(&menu),
            [
                "Needs you · 1",
                "Outreach",
                "Working · 1",
                "Reconciling October",
                "Idle · 1",
                "Shell · Drive",
            ]
        );
        assert_eq!((menu.min_width, menu.max_height), (300.0, 560.0));
        assert!(menu.quiet_labels);
        let outreach = &menu.entries[1];
        assert_eq!(
            (outreach.id.as_str(), outreach.kind),
            ("session:o", Kind::Item)
        );
        assert_eq!(outreach.detail, "Sales / Q4 leads");
        assert_eq!(
            outreach.dot,
            Some(Dot {
                colour: Token::StatusWarning,
                fade: 1.0,
                sign: crate::terminal::activity::Sign::Nudge,
            })
        );
        // A session reporting a task leads with it; its terminal's name moves below.
        assert_eq!(menu.entries[3].detail, "Finance · Bookkeeping");
        // An idle session is one line, with its dot.
        let shell = &menu.entries[5];
        assert_eq!(
            (shell.id.as_str(), shell.detail.as_str()),
            ("session:s", "")
        );
        assert_eq!(
            shell.dot.map(|dot| dot.colour),
            Some(Token::MutedForeground)
        );
        assert_eq!(session_entry("session:o"), Some("o"));
        assert_eq!(session_entry("next-session"), None);
        // The waiting sessions' heading shows the next-session shortcut without binding it;
        // a hidden row binds it.
        let heading = &menu.entries[0];
        assert_eq!(
            (heading.shortcut.as_str(), heading.key.as_str()),
            ("⌘⇧U", "")
        );
        let next = menu.entries.last().expect("next");
        assert_eq!(
            (next.id.as_str(), next.shortcut.as_str(), next.hidden),
            ("next-session", "⌘⇧U", true)
        );
        // With no session waiting, no heading shows the shortcut.
        let at_work = sessions_menu(&running[..1], "en");
        assert_eq!(titles(&at_work)[0], "Working · 1");
        assert_eq!(at_work.entries[0].shortcut, "");
        assert_eq!(
            bar_command("next-session"),
            Some(BarCommand::ShowNextSession)
        );
        // With no session running the menu says so. Review changes is not in it: it is the
        // title bar's own button.
        assert_eq!(titles(&sessions_menu(&[], "en"))[0], "No agents running");
        assert!(menu.find("changes").is_none());
        // The menu bar icon's menu lists the waiting sessions above its own rows.
        let tray = tray("Check for Updates…", true, &running, "en");
        assert_eq!(
            titles(&tray)[..3],
            ["Outreach · Needs you", "-", "Open Northroom"]
        );
        assert_eq!(tray.entries[0].id, "session:o");
    }

    /// Every key of the menu bar.
    #[test]
    fn the_menu_bar_keeps_every_key() {
        let mut keys = Vec::new();
        fn walk(menu: &Menu, keys: &mut Vec<String>) {
            for entry in &menu.entries {
                if !entry.key.is_empty() {
                    keys.push(format!("{} {} {}", entry.title, entry.key, entry.held));
                }
                if let Some(submenu) = &entry.submenu {
                    walk(submenu, keys);
                }
            }
        }
        menu_bar(STATE)
            .iter()
            .for_each(|menu| walk(menu, &mut keys));
        assert_eq!(
            keys,
            [
                "Settings… , 1",
                "Hide Northroom h 1",
                "Hide Others h 5",
                "Quit Northroom q 1",
                "Undo z 1",
                "Redo z 9",
                "Cut x 1",
                "Copy c 1",
                "Paste v 1",
                "Paste and Match Style v 13",
                "Select All a 1",
                "as Board 1 1",
                "as List 2 1",
                "as Columns 3 1",
                "Reload r 1",
                "Toggle Full Screen f 3",
                "Show or Hide Hidden Files . 9",
                "Command Palette k 1",
                "New Tab t 1",
                "Close Tab w 1",
                "Reopen Closed Tab t 9",
                "Back [ 1",
                "Forward ] 1",
                "Next Session That Needs You u 9",
                "Minimize m 1",
            ]
        );
    }

    #[test]
    fn the_view_choices_follow_the_folder_and_the_update_item_its_status() {
        let bar = menu_bar(STATE);
        let view = |bar: &[Menu], id: &str| {
            bar[2]
                .find(id)
                .map(|entry| (entry.enabled, entry.check))
                .expect("entry")
        };
        assert_eq!(view(&bar, "list"), (true, Check::On));
        assert_eq!(view(&bar, "board"), (true, Check::Off));
        let settings = menu_bar(BarState {
            folder_shows: false,
            ..STATE
        });
        assert_eq!(
            view(&settings, "list"),
            (false, Check::Off),
            "nothing is ticked while settings show"
        );
        let busy = menu_bar(BarState {
            update_label: "Downloading Update…",
            update_enabled: false,
            ..STATE
        });
        let updates = busy[0].find("updates").expect("updates");
        assert_eq!(
            (updates.title.as_str(), updates.enabled),
            ("Downloading Update…", false)
        );
        assert_eq!(bar_command("close-tab"), Some(BarCommand::CloseTab));
        assert_eq!(
            bar_command("columns"),
            Some(BarCommand::ShowView(View::Columns))
        );
        assert_eq!(bar_command("nothing"), None);
        assert_eq!(
            palette_command("drive.toggle-hidden-files"),
            Some(BarCommand::ToggleHiddenFiles)
        );
        // A check that names its own drive cannot change it.
        assert_eq!(bar_command("change-drive"), Some(BarCommand::ChooseDrive));
        assert_eq!(
            palette_command("drive.change-folder"),
            Some(BarCommand::ChooseDrive)
        );
        let fixed = menu_bar(BarState {
            drive_changeable: false,
            ..STATE
        });
        assert!(!fixed[0].find("change-drive").expect("the entry").enabled);
    }

    #[test]
    fn the_tray_follows_the_update_as_the_menu_bar_does() {
        let tray = tray("Install Downloaded Update…", true, &[], "en");
        assert_eq!(
            titles(&tray),
            [
                "Open Northroom",
                "-",
                "Install Downloaded Update…",
                "Quit Northroom"
            ]
        );
        assert_eq!(tray.entries[3].shortcut, "⌘Q");
        assert!(
            !self::tray("Checking for Updates…", false, &[], "en")
                .find("updates")
                .expect("updates")
                .enabled
        );
        assert_eq!(bar_command("open"), Some(BarCommand::ShowWindow));
    }

    /// A board card's menu: the file menu of that one file.
    fn card_menu(file: &File, view: View, desktop: Desktop, language: &str) -> Menu {
        file_menu(std::slice::from_ref(file), view, desktop, language)
    }

    #[test]
    fn a_files_menu_is_grouped_and_ordered_as_finders() {
        let picture = file("sky", None);
        let menu = card_menu(&picture, View::Board, Desktop::Mac, "en");
        assert_eq!(
            titles(&menu),
            [
                "Open",
                "Open in New Tab",
                "-",
                "Move to Trash",
                "-",
                "Rename",
                "Compress",
                "Duplicate",
                "-",
                "Copy",
                "Share…",
                "Show in Finder",
                "-",
                "View"
            ]
        );
        assert!(separators_are_tidy(&menu));
        assert!(
            menu.entries
                .iter()
                .filter(|entry| entry.kind == Kind::Item)
                .all(|entry| entry.icon.is_some() && entry.shortcut.is_empty()),
            "every entry has its glyph, so the words line up, and none shows keys, as Finder's"
        );
        assert!(menu.find("trash").expect("trash").destructive);
        let view = menu
            .entries
            .last()
            .and_then(|entry| entry.submenu.as_ref())
            .expect("view");
        assert_eq!((view.min_width, view.tight.clone()), (144.0, vec![1, 2]));
        assert_eq!(
            view.entries
                .iter()
                .map(|entry| (entry.shortcut.as_str(), entry.check))
                .collect::<Vec<_>>(),
            [("⌘1", Check::On), ("⌘2", Check::Off), ("⌘3", Check::Off)]
        );
    }

    #[test]
    fn a_protected_card_still_reads_and_says_why_it_takes_no_change() {
        let protected = card_menu(
            &file("Library", Some("protected")),
            View::Board,
            Desktop::Mac,
            "en",
        );
        assert_eq!(
            titles(&protected),
            [
                "Open",
                "Open in New Tab",
                "-",
                "Move to Trash",
                "-",
                "Rename",
                "Compress",
                "Duplicate",
                "-",
                "Copy",
                "Share…",
                "Show in Finder",
                "-",
                "protected",
                "-",
                "View"
            ]
        );
        assert!(separators_are_tidy(&protected));
        let enabled = |id: &str| protected.find(id).expect(id).enabled;
        assert_eq!(
            [
                "rename",
                "trash",
                "duplicate",
                "compress",
                "copy",
                "share",
                "reveal"
            ]
            .map(enabled),
            [false, false, true, true, true, true, true],
            "what only reads a protected file stays usable"
        );
    }

    #[test]
    fn each_desktop_names_its_file_manager_and_linux_has_no_share() {
        let reveal = |desktop| {
            let menu = card_menu(&file("a", None), View::Board, desktop, "en");
            assert!(separators_are_tidy(&menu));
            (
                menu.find("reveal").expect("reveal").title.clone(),
                menu.find("share").is_some(),
            )
        };
        assert_eq!(reveal(Desktop::Mac), ("Show in Finder".to_owned(), true));
        assert_eq!(
            reveal(Desktop::Windows),
            ("Show in File Explorer".to_owned(), true)
        );
        assert_eq!(reveal(Desktop::Linux), ("Show in Folder".to_owned(), false));
        let linux = file_menu(&[file("a", None)], View::List, Desktop::Linux, "en");
        assert_eq!(
            titles(&linux)[8..12],
            ["-", "Copy", "Show in Folder", "-"],
            "Copy and the reveal go together with no Share"
        );
        assert!(separators_are_tidy(&linux));
        assert_eq!(
            Desktop::this() == Desktop::Linux,
            cfg!(not(any(target_os = "macos", target_os = "windows")))
        );
    }

    #[test]
    fn a_rows_menu_is_about_the_group_it_is_part_of() {
        let selected = ["a".to_owned(), "b".to_owned()];
        assert_eq!(menu_group("b", &selected), ["a", "b"]);
        assert_eq!(
            menu_group("c", &selected),
            ["c"],
            "a row outside the group is about itself"
        );
        assert_eq!(menu_group("a", &selected[..1]), ["a"]);

        let one = file_menu(&[file("a", None)], View::List, Desktop::Mac, "en");
        assert_eq!(
            titles(&one),
            [
                "Open",
                "Open in New Tab",
                "-",
                "Move to Trash",
                "-",
                "Rename",
                "Compress",
                "Duplicate",
                "-",
                "Copy",
                "Share…",
                "Show in Finder",
                "-",
                "View"
            ],
            "it names neither the file nor how many, as Finder's does not"
        );
        assert!(separators_are_tidy(&one));
        assert_eq!(
            (one.min_width, one.entries[0].icon),
            (192.0, Some(Glyph::FolderOpen))
        );
        let group = file_menu(
            &[file("a", None), file("b", Some("protected"))],
            View::List,
            Desktop::Mac,
            "en",
        );
        assert_eq!(
            titles(&group),
            [
                "Move to Trash",
                "-",
                "Compress",
                "Duplicate",
                "-",
                "Copy",
                "Share…",
                "Show in Finder",
                "-",
                "protected",
                "-",
                "View"
            ]
        );
        assert!(separators_are_tidy(&group));
        assert!(
            !group.find("trash").expect("trash").enabled,
            "one protected file keeps the group out of the trash"
        );
        assert!(group.find("duplicate").expect("duplicate").enabled);
    }

    #[test]
    fn compress_writes_beside_the_first_file_only_where_its_folder_takes_files() {
        let compress = |menu: &Menu| menu.find("compress").expect("compress").enabled;
        let mut kept = file("a", None);
        kept.beside = Beside::Protected;
        assert!(!compress(&card_menu(
            &kept,
            View::Board,
            Desktop::Mac,
            "en"
        )));
        assert!(!compress(&file_menu(
            &[kept.clone(), file("b", None)],
            View::List,
            Desktop::Mac,
            "en"
        )));
        assert!(compress(&file_menu(
            &[file("b", None), kept],
            View::List,
            Desktop::Mac,
            "en"
        )));
        let free = card_menu(
            &file("a", Some("protected")),
            View::Board,
            Desktop::Mac,
            "en",
        );
        assert!(compress(&free), "a protected file is read, not changed");
        assert_eq!(
            (
                free.find("compress").expect("compress").shortcut.as_str(),
                folder_command("compress")
            ),
            ("", Some(FolderCommand::Compress)),
            "no keys"
        );
    }

    #[test]
    fn a_protected_folder_makes_nothing_new() {
        let open = background_menu(
            None,
            Clipboard::Files,
            View::Columns,
            Some(SortKey::Kind),
            "en",
        );
        assert_eq!(
            titles(&open),
            [
                "New page",
                "New folder",
                "New terminal",
                "-",
                "Import…",
                "Paste",
                "-",
                "Show Changes",
                "-",
                "View",
                "Sort By"
            ]
        );
        assert!(separators_are_tidy(&open));
        assert_eq!(
            folder_command("show-changes"),
            Some(FolderCommand::ShowChanges)
        );
        let paste = open.find("paste").expect("paste");
        assert_eq!(
            (paste.enabled, paste.shortcut.as_str(), paste.icon),
            (true, "", Some(Glyph::Clipboard))
        );
        let nothing_to_paste = background_menu(None, Clipboard::Other, View::Columns, None, "en");
        assert!(
            !nothing_to_paste.find("paste").expect("paste").enabled,
            "Paste waits for files on the clipboard"
        );
        let terminal = open.find("new-terminal").expect("new terminal");
        assert_eq!(
            (terminal.submenu.is_none(), terminal.icon),
            (true, Some(Glyph::TerminalWindow)),
            "one kind of terminal, with nothing to choose"
        );
        assert_eq!(open.min_width, 176.0);
        let protected = background_menu(
            Some("protected"),
            Clipboard::Files,
            View::Columns,
            None,
            "en",
        );
        assert_eq!(
            titles(&protected),
            [
                "New page",
                "New folder",
                "-",
                "Show Changes",
                "-",
                "View",
                "Sort By"
            ]
        );
        let sort = open.entries.last().and_then(|entry| entry.submenu.as_ref());
        let sort = sort.expect("Sort By");
        assert_eq!(
            titles(sort),
            [
                "Name",
                "Kind",
                "Date Modified",
                "Date Created",
                "Date Added",
                "Size"
            ]
        );
        let ticked: Vec<&str> = sort
            .entries
            .iter()
            .filter(|entry| entry.check == Check::On)
            .map(|entry| entry.title.as_str())
            .collect();
        assert_eq!(ticked, ["Kind"]);
        assert!(protected.entries[..2].iter().all(|entry| !entry.enabled));
    }

    #[test]
    fn a_folder_menus_ids_say_what_to_do() {
        assert_eq!(
            folder_command("view:2"),
            Some(FolderCommand::ShowView(View::Columns))
        );
        assert_eq!(folder_command("sort:4"), Some(FolderCommand::SortBy(4)));
        assert_eq!(
            folder_command("new-terminal"),
            Some(FolderCommand::NewTerminal)
        );
        assert_eq!(folder_command("import"), Some(FolderCommand::Import));
        assert_eq!(folder_command("view:x"), None);
        assert_eq!(folder_command("view:3"), None, "no view is numbered 3");
        assert_eq!(
            [
                "duplicate",
                "compress",
                "copy",
                "share",
                "reveal",
                "trash",
                "paste"
            ]
            .map(folder_command),
            [
                Some(FolderCommand::Duplicate),
                Some(FolderCommand::Compress),
                Some(FolderCommand::Copy),
                Some(FolderCommand::Share),
                Some(FolderCommand::Reveal),
                Some(FolderCommand::Trash),
                Some(FolderCommand::Paste),
            ]
        );
        // Every entry in a folder's menus maps to a command.
        let picture = file("sky", None);
        let menus = [
            card_menu(&picture, View::Board, Desktop::Mac, "en"),
            file_menu(
                std::slice::from_ref(&picture),
                View::List,
                Desktop::Mac,
                "en",
            ),
            background_menu(None, Clipboard::Files, View::Columns, None, "en"),
        ];
        for menu in &menus {
            for entry in menu.entries.iter().filter(|entry| entry.kind == Kind::Item) {
                if !entry.id.is_empty() {
                    assert!(folder_command(&entry.id).is_some(), "{}", entry.id);
                }
            }
        }
    }

    #[test]
    fn a_name_is_needed_where_one_is_asked() {
        assert_eq!(
            (
                rename_prompt("Notes", "en").title.as_str(),
                rename_prompt("Notes", "en").value.as_deref()
            ),
            ("Rename", Some("Notes"))
        );
        assert_eq!(rename_prompt("Notes", "en").action, "Save");
        assert_eq!(name_of("  Plan \n"), Some("Plan".to_owned()));
        assert_eq!(name_of(" \t\n"), None);
        assert_eq!(
            add_failed_detail("Claude Code", "en"),
            "Open Claude Code and try again."
        );
    }

    #[test]
    fn the_list_headers_options_list_every_row_in_order() {
        let choices = [
            (0, "Name", true),
            (3, "Size", true),
            (6, "Date Added", false),
        ];
        let moving = Some(Moving {
            key: 3,
            at: 0,
            count: 2,
        });
        let menu = list_options(moving, &choices, true, "en");
        assert_eq!(
            titles(&menu),
            [
                "Move column left",
                "Move column right",
                "-",
                "Show columns",
                "Name",
                "Size",
                "Date Added",
                "-",
                "Keep folders on top",
                "-",
                "Restore default view",
            ]
        );
        assert_eq!(menu.max_height, 448.0);
        assert_eq!(
            (
                menu.entries[0].enabled,
                menu.entries[1].enabled,
                menu.entries[1].id.as_str()
            ),
            (false, true, "move:2")
        );
        assert_eq!(
            (menu.entries[4].check, menu.entries[4].enabled),
            (Check::On, false),
            "Name stays"
        );
        assert_eq!(
            (menu.entries[6].check, menu.entries[6].keeps_open),
            (Check::Off, true)
        );
        let bare = list_options(None, &choices, false, "en");
        assert_eq!(titles(&bare)[..2], ["Show columns", "Name"]);
        assert_eq!(list_command("move:2"), Some(ListCommand::MoveColumn(2)));
        assert_eq!(list_command("column:6"), Some(ListCommand::ToggleColumn(6)));
        assert_eq!(list_command(""), None);
    }

    #[test]
    fn a_menu_finds_the_menu_holding_an_entry_in_itself_or_a_submenu() {
        let red = Entry::item("tag:red", "Red");
        let tags = Menu::new("Tags", vec![red]);
        let mut open = Entry::item("open", "Open");
        open.submenu = Some(tags);
        let menu = Menu::new("", vec![open]);
        assert_eq!(
            menu.holding("open").map(|menu| menu.title.as_str()),
            Some("")
        );
        assert_eq!(
            menu.holding("tag:red").map(|menu| menu.title.as_str()),
            Some("Tags")
        );
        assert!(menu.holding("nothing").is_none());
    }

    #[test]
    fn the_tray_ends_on_quit_after_a_separator() {
        let menu = tray("Check for Updates…", true, &[], "en");
        assert_eq!(bar_command(&menu.entries[3].id), Some(BarCommand::Quit));
        assert_eq!(menu.entries[3].key, "q");
        assert_eq!(menu.entries[1].kind, Kind::Separator);
        assert_eq!(menu.entries[0].check, Check::Off);
        assert!(menu.entries[0].submenu.is_none());
    }
    #[test]
    fn a_blocks_menu_copies_deletes_and_turns_words_into_other_blocks() {
        use crate::page::BlockKind;
        let paragraph = BlockKind::Paragraph {
            text: "Words".into(),
        };
        let words = page_block_menu(&paragraph, "en");
        assert_eq!(
            titles(&words),
            [
                "Duplicate",
                "Delete",
                "Turn into",
                "Text",
                "Heading 1",
                "Heading 2",
                "Heading 3",
                "Bulleted list",
                "Numbered list",
                "To-do list",
                "Quote",
                "Code"
            ]
        );
        assert_eq!(
            (words.min_width, words.icon_gap, words.quiet_labels),
            (224.0, 8.0, true)
        );
        let (duplicate, delete) = (&words.entries[0], &words.entries[1]);
        assert_eq!(
            (
                duplicate.icon,
                duplicate.shortcut.as_str(),
                duplicate.key.as_str(),
                duplicate.held
            ),
            (Some(Glyph::Copy), "⌘D", "d", COMMAND)
        );
        assert_eq!(
            (delete.icon, delete.shortcut.as_str(), delete.held),
            (Some(Glyph::Trash), "Del", 0)
        );
        assert_eq!(words.entries[2].kind, Kind::Label);
        let glyphs: Vec<Option<Glyph>> =
            words.entries[3..].iter().map(|entry| entry.icon).collect();
        assert_eq!(
            glyphs,
            [
                Glyph::TextT,
                Glyph::TextHOne,
                Glyph::TextHTwo,
                Glyph::TextHThree,
                Glyph::ListBullets,
                Glyph::ListNumbers,
                Glyph::CheckSquare,
                Glyph::Quotes,
                Glyph::Code
            ]
            .map(Some)
        );
        assert_eq!(words.entries[11].id, "turn:8");
        // Code and an HTML block still turn; a divider, a table, media and a player do not.
        assert_eq!(
            page_block_menu(&BlockKind::Html { text: "<b>".into() }, "en")
                .entries
                .len(),
            12
        );
        assert_eq!(
            page_block_menu(
                &BlockKind::Code {
                    language: None,
                    text: String::new()
                },
                "en"
            )
            .entries
            .len(),
            12
        );
        for kind in [
            BlockKind::Divider,
            BlockKind::Table {
                rows: vec![vec![String::new()]],
                column_widths_px: None,
            },
            BlockKind::Media {
                asset_id: "a".into(),
                caption: String::new(),
            },
            BlockKind::Player {
                html: "<video src=\"a.mp4\"></video>".into(),
            },
        ] {
            assert_eq!(
                titles(&page_block_menu(&kind, "en")),
                ["Duplicate", "Delete"]
            );
        }
        assert_eq!(
            page_block_command("duplicate"),
            Some(PageBlockCommand::Duplicate)
        );
        assert_eq!(page_block_command("delete"), Some(PageBlockCommand::Delete));
        assert_eq!(
            page_block_command("turn:6"),
            Some(PageBlockCommand::Turn(6))
        );
        assert_eq!(page_block_command("turn:11"), None);
        assert_eq!(page_block_command("turn:x"), None);
        assert_eq!(page_block_command("open"), None);
    }

    #[test]
    fn a_running_terminal_can_be_ended_from_its_menu_and_its_sessions_row() {
        let running = File {
            terminal: true,
            running: true,
            ..file("t", None)
        };
        let menu = file_menu(&[running], View::List, Desktop::Mac, "en");
        assert_eq!(
            titles(&menu)[..3],
            ["Open", "Open in New Tab", "End Session"]
        );
        assert_eq!(
            folder_command("end-session"),
            Some(FolderCommand::EndSession)
        );
        let folder = File {
            folder: true,
            ..file("f", None)
        };
        let menu = file_menu(&[folder], View::List, Desktop::Mac, "en");
        assert_eq!(
            titles(&menu)[..3],
            ["Open", "Open in New Tab", "Show Changes"]
        );
        assert_eq!(bar_command("changes"), Some(BarCommand::ShowChanges));
        let stopped = File {
            terminal: true,
            ..file("t", None)
        };
        let menu = file_menu(&[stopped], View::List, Desktop::Mac, "en");
        assert!(menu.find("end-session").is_none(), "nothing runs to end");

        let row = &sessions_menu(
            &[crate::sessions::Running {
                id: "t".to_owned(),
                name: "Terminal".to_owned(),
                folder: "Films".to_owned(),
                doing: String::new(),
                agent: String::new(),
                activity: crate::terminal::activity::Activity::Idle,
            }],
            "en",
        )
        .entries[1];
        assert_eq!(row.ends, "end-session:t");
        assert_eq!(end_session_entry(&row.ends), Some("t"));
        assert_eq!(end_session_entry("session:t"), None);
    }

    #[test]
    fn quitting_asks_first_only_while_sessions_work_or_wait() {
        assert!(quit_prompt(0, "en").is_none());
        let one = quit_prompt(1, "en").expect("asks");
        assert_eq!(one.title, "Quit Northroom?");
        assert_eq!(one.action, "Quit Northroom");
        assert!(one.destructive && one.value.is_none());
        let three = quit_prompt(3, "en").expect("asks");
        assert!(three.message.expect("says").starts_with("3 sessions"));
        assert_eq!(bar_command("quit"), Some(BarCommand::Quit));
    }

    #[test]
    fn ending_a_session_asks_first_only_while_it_works_or_waits() {
        use crate::terminal::activity::Activity;
        assert!(end_session_prompt("Terminal", Activity::Idle, "en").is_none());
        assert!(end_session_prompt("Terminal", Activity::Finished, "en").is_none());
        let working = end_session_prompt("Claude Code", Activity::Working, "en").expect("asks");
        assert_eq!(working.title, "End Claude Code?");
        assert_eq!(working.action, "End Session");
        assert!(working.destructive && working.value.is_none());
        let waiting = end_session_prompt("Claude Code", Activity::NeedsYou, "en").expect("asks");
        assert_ne!(waiting.message, working.message);
    }

    #[test]
    fn a_tabs_menu_closes_it_unless_it_is_the_last() {
        let menu = tab_menu(true, false, "en");
        assert_eq!(titles(&menu), ["Close tab", "Reopen closed tab"]);
        assert!(!menu.entries[1].enabled, "nothing closed yet");
        assert_eq!(menu.entries[1].shortcut, "⌘⇧T");
        assert!(tab_menu(true, true, "en").entries[1].enabled);
        assert_eq!(menu.min_width, 160.0);
        assert_eq!(
            (menu.entries[0].id.as_str(), menu.entries[0].enabled),
            (CLOSE_TAB_ENTRY, true)
        );
        assert!(
            !tab_menu(false, false, "en").entries[0].enabled,
            "the last tab stays"
        );
    }

    #[test]
    fn a_cells_menu_keeps_the_header_row_and_the_last_column() {
        let body = page_table_menu(2, 3, "en");
        assert_eq!(
            titles(&body),
            [
                "Add row below",
                "Add column",
                "-",
                "Delete row",
                "Delete column"
            ]
        );
        assert_eq!(body.entries[2].kind, Kind::Separator);
        let usable: Vec<bool> = body.entries.iter().map(|entry| entry.enabled).collect();
        assert_eq!(usable, [true, true, true, true, true]);
        let destructive: Vec<bool> = body.entries.iter().map(|entry| entry.destructive).collect();
        assert_eq!(destructive, [false, false, false, true, true]);
        assert!(
            !page_table_menu(0, 3, "en").entries[3].enabled,
            "the header row stays"
        );
        assert!(
            !page_table_menu(1, 1, "en").entries[4].enabled,
            "a table keeps one column"
        );
        assert_eq!(
            (body.min_width, body.icon_gap, body.quiet_labels),
            (0.0, 6.0, false)
        );
        for (id, command) in [
            ("add-row", PageTableCommand::AddRow),
            ("add-column", PageTableCommand::AddColumn),
            ("delete-row", PageTableCommand::DeleteRow),
            ("delete-column", PageTableCommand::DeleteColumn),
        ] {
            assert_eq!(page_table_command(id), Some(command));
        }
        assert_eq!(page_table_command("duplicate"), None);
    }

    #[test]
    fn the_words_are_in_the_interface_language() {
        let english = |key: Key| strings::text("en", key);
        let bar = menu_bar(BarState {
            language: "en",
            ..STATE
        });
        assert_eq!(bar[1].entries[0].title, english(key::MENU_UNDO));
        let card = card_menu(&file("sky", None), View::Board, Desktop::Mac, "en");
        assert_eq!(
            ["duplicate", "compress", "copy", "share", "reveal"].map(|id| card
                .find(id)
                .expect(id)
                .title
                .clone()),
            ["Duplicate", "Compress", "Copy", "Share…", "Show in Finder"],
            "the drive's new entries in Finder's own words"
        );
        assert_eq!(
            rename_prompt("Notes", "en").action,
            english(key::COMMON_SAVE)
        );
        assert_eq!(update_failed("en"), "Couldn’t update the file");
        assert_eq!(add_failed("en"), "Couldn’t add the files");
        assert_eq!(import_button("en"), "Import");
    }
}
