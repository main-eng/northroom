//! The FFI for a check's script (`script`). The shell asks what is due (`nr_script_next`) and
//! when to ask again; steps that only call the core run here, and the rest go to the shell as
//! typed steps. What the shell reads off its views (`nr_script_observe`, `nr_script_state`) is
//! merged here with core state, and the lines a check reads are written here. Also handles the
//! terminal's keys and wheel as the platform reports them (`terminal::input`). Returned text is
//! borrowed UTF-8, valid until the next call here that returns a step.

use crate::ffi::{text_of, NrKey, NrText};
use crate::Relock;

use crate::script::{
    self, ColumnsStep, Due, Event, Next, Plan, PreviewStep, Settle, Step, TerminalStep, Wheel,
};
use crate::terminal::input::Modifiers;
use std::ffi::{c_char, CString};
use std::sync::{LazyLock, Mutex};

/// What a run's views do differently for a check (`launch::Launch`).
#[repr(C)]
pub struct NrScriptRun {
    /// The steps include real events, so the window acts as the key window and every view accepts
    /// presses.
    pub scripted: bool,
    /// The terminal is shown (`NR_NO_TERMINAL` leaves it out).
    pub terminal: bool,
}

#[no_mangle]
pub extern "C" fn nr_script_run() -> NrScriptRun {
    let launch = crate::launch::get();
    NrScriptRun {
        scripted: launch.scripted,
        terminal: launch.terminal,
    }
}

// MARK: Deferred work

/// Work a step defers.
#[derive(Clone, Debug, PartialEq)]
enum Later {
    /// Check the columns' or board's scroll again (`Settle`).
    Look,
    /// A screenshot taken once queued events are handled and drawn.
    Shot {
        path: String,
        alone: bool,
    },
    /// Screenshots of the open menus, named `<stem><n>.png`, once drawn.
    SaveMenus(String),
    /// The open menus' positions and screenshots.
    ReportMenus {
        tag: String,
        stem: String,
    },
    /// Wait until a downloaded update is ready.
    WaitForUpdate,
    Quit,
}

/// Delay before a window screenshot, so queued events are handled and drawn.
const SHOT_AFTER: f64 = 0.35;
/// A menu just opened is drawn after this long.
const MENU_DRAWN_AFTER: f64 = 0.3;
/// How often a quit waiting on a downloaded update checks again.
const UPDATE_LOOK: f64 = 0.2;
const QUIT_AFTER: f64 = 1.0;

/// The scroller a look waits on to settle.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Scrolled {
    Columns,
    Board,
    /// The scroller under the last point.
    Under,
}

struct Runner {
    plan: Plan<Later>,
    settle: Settle,
    /// The scroller the current look is watching.
    looking: Scrolled,
    /// The `at:` step whose point the shell is locating: its event name and the row name.
    at: Option<(String, String)>,
    /// The last monotonic time from the shell, for scheduling deferred work.
    now: f64,
    lent: (String, String),
}

static RUNNER: LazyLock<Mutex<Runner>> = LazyLock::new(|| {
    Mutex::new(Runner {
        plan: Plan::new(script::get()),
        settle: Settle::default(),
        looking: Scrolled::Columns,
        at: None,
        now: 0.0,
        lent: (String::new(), String::new()),
    })
});

fn runner() -> std::sync::MutexGuard<'static, Runner> {
    RUNNER.relock()
}

// MARK: Steps for the shell

/// What the shell does for a step.
#[derive(Clone, Debug, PartialEq)]
enum Shell {
    Nothing,
    Event(Event),
    /// A window screenshot, taken now.
    Shot {
        path: String,
        alone: bool,
    },
    /// Report how far the columns or the board are scrolled (`nr_script_looked`).
    LookAt(Scrolled),
    /// Locate the row or card with that name (`nr_script_at`).
    At(String),
    MenuBar,
    MenuKeys,
    TrayMenu,
    Gutter {
        plus: bool,
    },
    CardMenu,
    BackgroundMenu,
    SaveMenus(String),
    ReportMenus {
        tag: String,
        stem: String,
    },
    Dialog,
    /// Start an inline rename of the file with this id.
    Rename(String),
    EditFirst,
    Media(String),
    OpenLogs,
    /// Check for Updates… (`true`), or a scheduled check.
    UpdateCheck(bool),
    UpdateChanged,
    Quit,
    ThemeChanged,
    /// Set the app's appearance (`Some(true)` for dark), or follow the system again (`None`).
    Look(Option<bool>),
    PageReloaded,
    HtmlProbe,
    RightClick {
        x: f64,
        y: f64,
        tag: String,
        stem: String,
    },
    Columns(ColumnsStep, String),
    MenuRow(i64),
    /// The preview step, with its audio, after reporting the page players' state (`page`).
    Preview {
        page: bool,
    },
    TerminalTyping,
    Trace,
    Window(String),
}

/// The kind of step the shell performs (`NrScriptStep::kind`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrScriptKind {
    Nothing,
    /// An event (`event` says which).
    Event,
    /// Find the row or card `name` and call `nr_script_at`.
    At,
    /// Report the menu bar's titles.
    MenuBar,
    /// Report the menu bar's shortcuts.
    MenuKeys,
    /// Report the tray's menu.
    TrayMenu,
    /// The page's block gutter on row 1 (`number` 1 for its "+").
    Gutter,
    /// Open the board's third card's menu.
    CardMenu,
    /// Open the folder's background menu.
    BackgroundMenu,
    /// Save screenshots of the open menus as `text<n>.png`.
    SaveMenus,
    /// Report the open menus' positions (tagged `text`) and save screenshots as `name<n>.png`
    /// (none when `name` is empty).
    ReportMenus,
    /// Show the dialog `nr_prompt` describes.
    Dialog,
    /// Start an inline rename of the file with id `text` (`NrEffect::Rename`).
    Rename,
    /// Start typing in the page's first block.
    EditFirst,
    /// The page's media step `name` on block `number`.
    Media,
    /// Open the diagnostic logs' folder.
    OpenLogs,
    /// Check for updates (`number` 1 for a manual check, 0 for a scheduled one).
    UpdateCheck,
    /// The update's button changed.
    UpdateChanged,
    Quit,
    /// The appearance changed.
    ThemeChanged,
    /// Reload the page.
    PageReloaded,
    /// Report what the open HTML page shows.
    HtmlProbe,
    /// A right click at `x`, `y`, tagged `text`, with menu screenshots saved as `name<n>.png`.
    RightClick,
    /// A columns step (`columns`; `number` is the column, -1 for none; `x`..`y` the rows, -1 for
    /// none; `key`, `modifiers`), reported as `name`.
    Columns,
    /// Rest the pointer on row `number` of the last open menu.
    MenuRow,
    /// The preview step `name`, with its audio (`previewpage` reports the page players' state
    /// first).
    Preview,
    /// Press `NR_TERM_TYPE`'s keys (`nr_script_press`) on the terminal.
    TerminalTyping,
    /// Report the window's layout.
    Trace,
    /// Save a window screenshot to `text`.
    Window,
    /// Set the app's appearance as the system would (`number` 0 light, 1 dark, -1 follow the
    /// system again).
    Look,
}

/// An event a step makes (`NrScriptStep::event`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrScriptEvent {
    /// Not an event.
    None,
    /// Move to `x`, `y`.
    Move,
    /// The button down (`modifiers`).
    Down,
    Up,
    /// A click (`modifiers`).
    Click,
    /// A double click (`modifiers`).
    DoubleClick,
    RightClick,
    MiddleClick,
    /// A precise wheel step of `dy` down and `dx` across (`modifiers`).
    Precise,
    /// A mouse wheel click.
    Lines,
    /// A two-finger swipe.
    Swipe,
    /// A two-finger pinch that spreads the fingers by `dy` (pinches in when negative).
    Pinch,
    /// Report the tabs' scroll.
    Tabs,
    /// `nr_script_observe`.
    Observe,
    /// `nr_script_state`.
    State,
    /// The key `key` (`Other`: the character `text`) with `modifiers`.
    Key,
    /// Type `text`.
    Text,
    /// A screenshot to `text` (`alone` for the window only).
    Shot,
    /// A system drag from `x`, `y` to `dx`, `dy`.
    DragCheck,
    /// The file `text` dropped at `x`, `y` from another app.
    Drop,
    /// Report what is under the last point.
    Say,
    /// Report the columns' scroll (`nr_script_looked`).
    LookColumns,
    /// Report the board's scroll (likewise).
    LookBoard,
    /// Report the scroll of the scroller under the last point (likewise).
    LookUnder,
}

/// A columns' step (`NrScriptStep::columns`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NrColumnsScript {
    None,
    Focus,
    Group,
    Key,
    /// Report only.
    Say,
}

/// A step for the shell. Modifiers are `NR_HELD_*`.
#[repr(C)]
pub struct NrScriptStep {
    /// Milliseconds until the shell asks again.
    pub delay_ms: u32,
    /// The script is finished; nothing more will be due.
    pub over: bool,
    pub kind: NrScriptKind,
    pub event: NrScriptEvent,
    pub columns: NrColumnsScript,
    /// Redraw the window from core state once the step is done.
    pub sync: bool,
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
    pub modifiers: u32,
    pub key: NrKey,
    pub number: i64,
    pub alone: bool,
    pub text: NrText,
    pub name: NrText,
}

struct Handed {
    kind: NrScriptKind,
    event: NrScriptEvent,
    columns: NrColumnsScript,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    modifiers: u32,
    key: NrKey,
    number: i64,
    alone: bool,
    text: String,
    name: String,
}

impl Handed {
    fn kind(kind: NrScriptKind) -> Self {
        Self {
            kind,
            event: NrScriptEvent::None,
            columns: NrColumnsScript::None,
            x: 0.0,
            y: 0.0,
            dx: 0.0,
            dy: 0.0,
            modifiers: 0,
            key: NrKey::Other,
            number: 0,
            alone: false,
            text: String::new(),
            name: String::new(),
        }
    }

    fn text(kind: NrScriptKind, text: &str) -> Self {
        Self {
            text: text.to_owned(),
            ..Self::kind(kind)
        }
    }

    /// An event at no point.
    fn made(event: NrScriptEvent) -> Self {
        Self {
            event,
            ..Self::kind(NrScriptKind::Event)
        }
    }

    fn event(event: NrScriptEvent, at: script::Point) -> Self {
        Self {
            x: at.x,
            y: at.y,
            ..Self::made(event)
        }
    }
}

fn handed(shell: Shell) -> Handed {
    use Handed as H;
    match shell {
        Shell::Nothing => H::kind(NrScriptKind::Nothing),
        Shell::Event(event) => handed_event(event),
        Shell::Shot { path, alone } => H {
            alone,
            text: path,
            ..H::made(NrScriptEvent::Shot)
        },
        Shell::LookAt(Scrolled::Columns) => H::made(NrScriptEvent::LookColumns),
        Shell::LookAt(Scrolled::Board) => H::made(NrScriptEvent::LookBoard),
        Shell::LookAt(Scrolled::Under) => H::made(NrScriptEvent::LookUnder),
        Shell::At(name) => H {
            name,
            ..H::kind(NrScriptKind::At)
        },
        Shell::MenuBar => H::kind(NrScriptKind::MenuBar),
        Shell::MenuKeys => H::kind(NrScriptKind::MenuKeys),
        Shell::TrayMenu => H::kind(NrScriptKind::TrayMenu),
        Shell::Gutter { plus } => H {
            number: i64::from(plus),
            ..H::kind(NrScriptKind::Gutter)
        },
        Shell::CardMenu => H::kind(NrScriptKind::CardMenu),
        Shell::BackgroundMenu => H::kind(NrScriptKind::BackgroundMenu),
        Shell::SaveMenus(stem) => H::text(NrScriptKind::SaveMenus, &stem),
        Shell::ReportMenus { tag, stem } => H {
            name: stem,
            ..H::text(NrScriptKind::ReportMenus, &tag)
        },
        Shell::Dialog => H::kind(NrScriptKind::Dialog),
        Shell::Rename(id) => H::text(NrScriptKind::Rename, &id),
        Shell::EditFirst => H::kind(NrScriptKind::EditFirst),
        Shell::Media(name) => H {
            name,
            number: script::get().block as i64,
            ..H::kind(NrScriptKind::Media)
        },
        Shell::OpenLogs => H::kind(NrScriptKind::OpenLogs),
        Shell::UpdateCheck(manual) => H {
            number: i64::from(manual),
            ..H::kind(NrScriptKind::UpdateCheck)
        },
        Shell::UpdateChanged => H::kind(NrScriptKind::UpdateChanged),
        Shell::Quit => H::kind(NrScriptKind::Quit),
        Shell::ThemeChanged => H::kind(NrScriptKind::ThemeChanged),
        Shell::PageReloaded => H::kind(NrScriptKind::PageReloaded),
        Shell::HtmlProbe => H::kind(NrScriptKind::HtmlProbe),
        Shell::RightClick { x, y, tag, stem } => H {
            x,
            y,
            name: stem,
            ..H::text(NrScriptKind::RightClick, &tag)
        },
        Shell::Columns(step, words) => handed_columns(step, words),
        Shell::MenuRow(row) => H {
            number: row,
            ..H::kind(NrScriptKind::MenuRow)
        },
        Shell::Preview { page } => H {
            name: if page { "previewpage" } else { "" }.to_owned(),
            ..H::kind(NrScriptKind::Preview)
        },
        Shell::TerminalTyping => H::kind(NrScriptKind::TerminalTyping),
        Shell::Trace => H::kind(NrScriptKind::Trace),
        Shell::Window(path) => H::text(NrScriptKind::Window, &path),
        Shell::Look(dark) => H {
            number: dark.map_or(-1, i64::from),
            ..H::kind(NrScriptKind::Look)
        },
    }
}

/// The shell step for a scripted event.
fn handed_event(event: Event) -> Handed {
    use Handed as H;
    match event {
        Event::Move(at) => H::event(NrScriptEvent::Move, at),
        Event::Down(at, held) => H {
            modifiers: held,
            ..H::event(NrScriptEvent::Down, at)
        },
        Event::Up(at) => H::event(NrScriptEvent::Up, at),
        Event::Click(at, held) => H {
            modifiers: held,
            ..H::event(NrScriptEvent::Click, at)
        },
        Event::DoubleClick(at, held) => H {
            modifiers: held,
            ..H::event(NrScriptEvent::DoubleClick, at)
        },
        Event::RightClick(at) => H::event(NrScriptEvent::RightClick, at),
        Event::MiddleClick(at) => H::event(NrScriptEvent::MiddleClick, at),
        Event::Scroll {
            at,
            down,
            across,
            held,
            wheel,
        } => H {
            dx: across,
            dy: down,
            modifiers: held,
            ..H::event(wheel_event(wheel), at)
        },
        Event::Pinch { at, by } => H {
            dy: by,
            ..H::event(NrScriptEvent::Pinch, at)
        },
        Event::Tabs => H::made(NrScriptEvent::Tabs),
        Event::Observe => H::made(NrScriptEvent::Observe),
        Event::State => H::made(NrScriptEvent::State),
        Event::Key {
            key,
            held,
            characters,
        } => H {
            key: script::key_code(key),
            modifiers: held,
            text: characters,
            ..H::made(NrScriptEvent::Key)
        },
        Event::Text(text) => H {
            text,
            ..H::made(NrScriptEvent::Text)
        },
        Event::DragCheck { from, to } => H {
            dx: to.x,
            dy: to.y,
            ..H::event(NrScriptEvent::DragCheck, from)
        },
        Event::Drop { at, path } => H {
            text: path,
            ..H::event(NrScriptEvent::Drop, at)
        },
        Event::Say => H::made(NrScriptEvent::Say),
        // Each defers work or does nothing; none reaches the shell.
        Event::Shot { .. } | Event::Columns | Event::Board | Event::Scrolled | Event::Wait => {
            H::kind(NrScriptKind::Nothing)
        }
    }
}

/// The event for a scroll by `wheel`.
fn wheel_event(wheel: Wheel) -> NrScriptEvent {
    match wheel {
        Wheel::Precise => NrScriptEvent::Precise,
        Wheel::Lines => NrScriptEvent::Lines,
        Wheel::Swipe => NrScriptEvent::Swipe,
    }
}

/// The shell step for a columns step, reported as `words`.
fn handed_columns(step: ColumnsStep, words: String) -> Handed {
    use Handed as H;
    let columns = |step: NrColumnsScript, column: Option<usize>| H {
        columns: step,
        number: column.map_or(-1, |column| column as i64),
        x: -1.0,
        y: -1.0,
        name: words.clone(),
        ..H::kind(NrScriptKind::Columns)
    };
    match step {
        ColumnsStep::Focus(column) => columns(NrColumnsScript::Focus, column),
        ColumnsStep::Group(column, rows) => {
            let (first, last) =
                rows.map_or((-1.0, -1.0), |(first, last)| (first as f64, last as f64));
            H {
                x: first,
                y: last,
                ..columns(NrColumnsScript::Group, column)
            }
        }
        ColumnsStep::Key(column, key) => match key {
            Some((key, held)) => H {
                key: script::key_code(key),
                modifiers: held,
                ..columns(NrColumnsScript::Key, column)
            },
            None => columns(NrColumnsScript::Say, None),
        },
        ColumnsStep::Say => columns(NrColumnsScript::Say, None),
    }
}

fn hand(runner: &mut Runner, shell: Shell, sync: bool, delay: f64, over: bool) -> NrScriptStep {
    let handed = handed(shell);
    runner.lent = (handed.text, handed.name);
    NrScriptStep {
        delay_ms: (delay * 1000.0).ceil() as u32,
        over,
        kind: handed.kind,
        event: handed.event,
        columns: handed.columns,
        sync,
        x: handed.x,
        y: handed.y,
        dx: handed.dx,
        dy: handed.dy,
        modifiers: handed.modifiers,
        key: handed.key,
        number: handed.number,
        alone: handed.alone,
        text: NrText::borrowing(&runner.lent.0),
        name: NrText::borrowing(&runner.lent.1),
    }
}

// MARK: Core steps

/// Changes the open page as an edit would: records one undo step if `edit` reports a change,
/// and saves.
fn change_page(edit: impl FnOnce(&mut crate::page::Page) -> bool) {
    crate::ffi::edit_page(|page| (edit(page), ()));
}

fn c(text: &str) -> CString {
    CString::new(text.replace('\0', "")).expect("no NUL is left")
}

/// Copies text another call just lent.
fn owned(text: NrText) -> String {
    // SAFETY: the text was lent just now and nothing has lent since.
    unsafe { text.to_owned_string() }
}

/// The file stem for menu screenshots, when the run saves screenshots.
fn menu_stem(name: &str) -> Option<String> {
    script::get().picture_path(name)
}

/// Saves screenshots of the menus opening now, once drawn, alongside the run's other
/// screenshots (names starting with `prefix`), when the run saves screenshots.
fn save_menus_when_drawn(runner: &mut Runner, prefix: &str) {
    if let Some(stem) = menu_stem(prefix) {
        let now = runner.now;
        runner
            .plan
            .after(now, MENU_DRAWN_AFTER, Due::Later(Later::SaveMenus(stem)));
    }
}

/// Opens Connect Device's dialog, for the shell to show when the link started.
fn pairing_dialog() -> Shell {
    if crate::menus::ffi::prompt_pairing() {
        Shell::Dialog
    } else {
        Shell::Nothing
    }
}

/// Runs the core's part of `step` and returns the shell's part.
fn run_step(runner: &mut Runner, step: Step) -> Shell {
    use crate::ffi::*;
    let script = script::get();
    let now = runner.now;
    match step {
        Step::Back => nr_history_back(),
        Step::Forward => nr_history_forward(),
        Step::NewTab => nr_tab_open(),
        Step::CloseTab => nr_tab_close(nr_tabs_active()),
        Step::FirstTab => nr_tab_select(0),
        Step::Settings => nr_settings_open(),
        Step::MenuBar => return Shell::MenuBar,
        Step::MenuKeys => return Shell::MenuKeys,
        Step::Gutter { plus } => return Shell::Gutter { plus },
        Step::Ink => {
            // SAFETY: a NUL-terminated string.
            unsafe {
                crate::app_ffi::nr_settings_act(
                    c("appearance.dark.preset").as_ptr(),
                    c("ink").as_ptr(),
                )
            };
            return Shell::ThemeChanged;
        }
        Step::Look(dark) => return Shell::Look(dark),
        Step::Theme => eprintln!("{}", theme_line()),
        Step::Expand => expand_two_folders(),
        Step::CardMenu => {
            save_menus_when_drawn(runner, "menu-");
            return Shell::CardMenu;
        }
        Step::BackgroundMenu => {
            save_menus_when_drawn(runner, "background-menu-");
            return Shell::BackgroundMenu;
        }
        Step::New(command) => {
            let column = nr_shown_column();
            crate::menus::ffi::menu_folder(crate::menus::ffi::NrMenu::Background, column, "", &[]);
            let command = c(&command);
            // SAFETY: a NUL-terminated command.
            let effect = unsafe {
                crate::menus::ffi::nr_menu_run(
                    crate::menus::ffi::NrMenu::Background,
                    command.as_ptr(),
                )
            };
            if effect.effect == crate::menus::ffi::NrEffect::Rename {
                return Shell::Rename(owned(effect.text));
            }
        }
        Step::Dialog => {
            let column = nr_shown_column();
            let first = with_columns(|columns| {
                Some(columns.columns().get(column)?.rows.first()?.id.clone())
            })
            .flatten();
            if let Some(id) = first.map(|id| c(&id)) {
                // SAFETY: a NUL-terminated id.
                if unsafe { crate::menus::ffi::nr_prompt_rename(column, id.as_ptr()) } {
                    // The core's naming dialog is ready.
                    return Shell::Dialog;
                }
            }
        }
        Step::Phone => return pairing_dialog(),
        Step::View(view) => set_folder_view(view),
        Step::Sort(key) => list_toggle_sort(key),
        Step::ListColumns(keys, width) => {
            set_folder_view(crate::columns::View::List);
            keys.iter().for_each(|key| list_toggle_column(*key));
            if let Some((key, width)) = width {
                list_set_width(key, width);
            }
        }
        Step::EditFirst => return Shell::EditFirst,
        Step::BoldWord => {
            change_page(|page| {
                page.toggle_mark(0, 0, 7, crate::page::inline_edit::Construct::Bold)
                    .is_some()
            });
            return Shell::PageReloaded;
        }
        Step::PasteBlocks => {
            change_page(|page| {
                page.insert_markdown_after(0, "## Pasted\n\n- one\n- two")
                    .is_some()
            });
            return Shell::PageReloaded;
        }
        Step::TableRow => {
            change_page(|page| page.add_table_row(script.block, 2));
            change_page(|page| page.add_table_column(script.block));
            change_page(|page| page.set_table_cell(script.block, 3, 0, "gamma"));
            change_page(|page| page.set_table_cell(script.block, 0, 2, "Note"));
            return Shell::PageReloaded;
        }
        Step::Caption => {
            change_page(|page| page.set_caption(script.block, &script.text));
            return Shell::PageReloaded;
        }
        Step::Media(name) => return Shell::Media(name),
        Step::Tick => change_page(|page| page.toggle_todo(script.block)),
        Step::Retype => change_page(|page| page.set_text(script.block, &script.text)),
        Step::Logs => eprintln!("logs\t{}", owned(nr_log_directory())),
        Step::OpenLogs => return Shell::OpenLogs,
        Step::UpdateCheck(manual) => return Shell::UpdateCheck(manual),
        Step::UpdateQuitWhenReady => wait_for_update(runner),
        Step::UpdateStatus(status) => {
            crate::update::ffi::picture(&status);
            return Shell::UpdateChanged;
        }
        Step::TrayMenu => return Shell::TrayMenu,
        Step::Act(action, value) => {
            // SAFETY: NUL-terminated strings.
            unsafe { crate::app_ffi::nr_settings_act(c(&action).as_ptr(), c(&value).as_ptr()) };
            return Shell::ThemeChanged;
        }
        Step::Event(event) => return event_step(runner, event),
        Step::At(event, name) => {
            runner.at = Some((event, name.clone()));
            return Shell::At(name);
        }
        Step::AtNothing(words) => eprintln!("at\tno row or card named {words}"),
        Step::HtmlProbe => return Shell::HtmlProbe,
        Step::RightClick(at) => {
            let tag = format!("rightclick {} {}", at.x as i64, at.y as i64);
            let stem = menu_stem(&format!("{}-menu-", tag.replace(' ', "-"))).unwrap_or_default();
            return Shell::RightClick {
                x: at.x,
                y: at.y,
                tag,
                stem,
            };
        }
        Step::Columns(step, words) => return Shell::Columns(step, words),
        Step::MenuRow(row) => {
            let stem = menu_stem("menurow-menu-").unwrap_or_default();
            runner.plan.after(
                now,
                MENU_DRAWN_AFTER,
                Due::Later(Later::ReportMenus {
                    tag: "menurow".to_owned(),
                    stem,
                }),
            );
            return Shell::MenuRow(row);
        }
        Step::Preview(step) => return preview_step(step),
        Step::Terminal(step) => return terminal_step(step),
        Step::Nothing => {}
    }
    Shell::Nothing
}

/// Opens the first two folders of the list in place.
fn expand_two_folders() {
    use crate::ffi::*;
    set_folder_view(crate::columns::View::List);
    let mut opened = 0;
    let mut index = 0;
    nr_list_load();
    while opened < 2 {
        let row = app_list_row(index);
        let Some((id, folder, expanded, placeholder, depth)) = row else {
            break;
        };
        if folder && !expanded && !placeholder && depth == 0 {
            let id = c(&id);
            // SAFETY: a NUL-terminated string.
            unsafe { nr_list_toggle_expanded(id.as_ptr()) };
            nr_list_load();
            opened += 1;
        }
        index += 1;
    }
}

/// Row `index` of the list as last loaded: file id, whether it is a folder, whether it is
/// expanded, whether it is a placeholder for an expanded folder's files, and its depth.
fn app_list_row(index: usize) -> Option<(String, bool, bool, bool, u32)> {
    let rows = crate::app().list_rows.relock();
    let row = rows.get(index)?;
    Some((
        row.entry.id.clone(),
        row.entry.kind == crate::workspace::EntryKind::Folder,
        row.expanded,
        row.placeholder.is_some(),
        row.depth,
    ))
}

/// The first column's rows: title and id, in order.
fn first_column_rows() -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut index = 0;
    loop {
        // SAFETY: storage for one row; its text is copied before the next call.
        let mut row = unsafe { std::mem::zeroed::<crate::ffi::NrFileRow>() };
        if !unsafe { crate::ffi::nr_column_row(0, index, &mut row) } {
            return rows;
        }
        rows.push((owned(row.title), owned(row.id)));
        index += 1;
    }
}

/// Quits as a person would once a downloaded update is ready (and Later was pressed), so the
/// install-on-quit path can be checked.
fn wait_for_update(runner: &mut Runner) {
    let later = if crate::update::ffi::update_ready() {
        (QUIT_AFTER, Later::Quit)
    } else {
        (UPDATE_LOOK, Later::WaitForUpdate)
    };
    let now = runner.now;
    runner.plan.after(now, later.0, Due::Later(later.1));
}

fn event_step(runner: &mut Runner, event: Event) -> Shell {
    let now = runner.now;
    match event {
        Event::Shot { name, alone } => {
            if let Some(path) = script::get().picture_path(&format!("{name}.png")) {
                runner
                    .plan
                    .after(now, SHOT_AFTER, Due::Later(Later::Shot { path, alone }));
            }
            Shell::Nothing
        }
        Event::Columns => look_at(runner, Scrolled::Columns),
        Event::Board => look_at(runner, Scrolled::Board),
        Event::Scrolled => look_at(runner, Scrolled::Under),
        Event::Wait => Shell::Nothing,
        event => Shell::Event(event),
    }
}

/// Makes later steps wait for the columns or the board to stop scrolling.
fn look_at(runner: &mut Runner, scrolled: Scrolled) -> Shell {
    runner.plan.hold(true);
    runner.settle = Settle::default();
    runner.looking = scrolled;
    Shell::LookAt(scrolled)
}

/// The effective theme, the one views are drawn in, and the choice in Settings.
fn theme_line() -> String {
    use crate::settings::appearance::Choice;
    let (dark, background) = {
        let theme = crate::app().theme.relock();
        (
            theme.mode == crate::theme::Mode::Dark,
            crate::theme::oklch_to_srgb8(theme.background),
        )
    };
    let choice = match crate::ffi::with_appearance(|settings| settings.shown_choice()) {
        Choice::Light => "light",
        Choice::Dark => "dark",
        Choice::System => "system",
    };
    script::theme_line(dark, background, choice)
}

fn preview_step(step: PreviewStep) -> Shell {
    use crate::ffi::*;
    use crate::preview::Preview;
    match step {
        PreviewStep::Play => {
            with_preview(Preview::toggle_playing);
        }
        PreviewStep::Mute => {
            let muted = nr_preview_info().muted;
            with_preview(|preview| preview.set_muted(!muted));
        }
        PreviewStep::Half => {
            with_preview(|preview| preview.seek_share(0.5));
        }
        PreviewStep::Drag => {
            // A scrub drag: playback pauses for its duration.
            with_preview(Preview::begin_scrub);
            for step in 0..=10 {
                with_preview(|preview| preview.seek_share(0.25 + f64::from(step) * 0.05));
            }
            with_preview(Preview::end_scrub);
        }
        PreviewStep::Stats => {
            let info = nr_preview_info();
            let sound = with_preview(Preview::audio_stats).unwrap_or_default();
            let clock =
                crate::preview::player::clock_text(info.position_seconds, info.duration_seconds);
            eprintln!(
                "{}",
                script::preview_line(&script::PreviewStats {
                    clock: &clock,
                    playing: info.playing,
                    ended: info.ended,
                    position: info.position_seconds,
                    duration: info.duration_seconds,
                    width: info.width,
                    height: info.height,
                    audio: info.has_audio,
                    muted: info.muted,
                    volume: info.volume,
                    rate: info.rate,
                    fullscreen: info.fullscreen,
                    frames: sound.frames,
                    missing: sound.missing_frames,
                    resyncs: sound.resyncs,
                    eased: sound.eased_frames,
                    peak: sound.peak,
                    at: sound.position_seconds,
                    drift: sound.drift_seconds,
                    largest: sound.largest_drift_seconds,
                })
            );
        }
        PreviewStep::Page => return Shell::Preview { page: true },
        PreviewStep::Use => list_open(0),
        PreviewStep::Rate(rate) => preview_set_rate(rate),
        PreviewStep::Key(key) => {
            if let Some(key) = crate::preview::player::Key::from_press(key) {
                with_preview(|preview| preview.key(key));
            }
        }
        PreviewStep::Nothing => {}
    }
    Shell::Preview { page: false }
}

/// Terminal steps call the core as the view does but never touch the person's pasteboard; a
/// copy is written to `copied.txt` beside the screenshots instead.
fn terminal_step(step: TerminalStep) -> Shell {
    use crate::ffi::*;
    let settings = &script::get().terminal;
    // A check's steps go to the terminal that has the keyboard.
    let front = front_terminal();
    match step {
        TerminalStep::Focus(focused) => nr_terminal_focus(front, focused),
        TerminalStep::Drag => {
            if let [x0, y0, x1, y1, ref rest @ ..] = settings.drag[..] {
                let clicks = rest.first().map_or(1, |clicks| *clicks as u32);
                nr_terminal_mouse(
                    front,
                    NrMouseAction::Pressed,
                    NrMouseButton::Left,
                    x0,
                    y0,
                    clicks,
                    0,
                );
                nr_terminal_mouse(
                    front,
                    NrMouseAction::Dragged,
                    NrMouseButton::Left,
                    x1,
                    y1,
                    clicks,
                    0,
                );
                nr_terminal_mouse(
                    front,
                    NrMouseAction::Released,
                    NrMouseButton::Left,
                    x1,
                    y1,
                    clicks,
                    0,
                );
            }
        }
        TerminalStep::Copy => {
            if let Some(path) = script::get().picture_path("copied.txt") {
                if let Err(error) = std::fs::write(&path, owned(nr_terminal_copy(front))) {
                    eprintln!("{path} was not written: {error}");
                }
            }
        }
        // SAFETY: the bytes of a string that lives as long as the process.
        TerminalStep::Paste => unsafe {
            nr_terminal_paste(front, settings.paste.as_ptr(), settings.paste.len())
        },
        TerminalStep::Refer => {
            // A title listed twice names its last row.
            let ids: std::collections::HashMap<String, String> =
                first_column_rows().into_iter().collect();
            if let Some(terminal) = settings.refer.first().and_then(|title| ids.get(title)) {
                let files: Vec<String> = settings.refer[1..]
                    .iter()
                    .filter_map(|title| ids.get(title))
                    .cloned()
                    .collect();
                let referred = reference_files(terminal, &files);
                eprintln!("termrefer\t{referred}");
            }
        }
        TerminalStep::Scroll => terminal_scroll(
            front,
            settings.scroll.first().copied().unwrap_or(0.0),
            true,
            10.0,
            10.0,
            Modifiers(0),
        ),
        // SAFETY: the bytes of a string that lives as long as the process.
        TerminalStep::Compose => unsafe {
            nr_terminal_compose(front, settings.compose.as_ptr(), settings.compose.len())
        },
        TerminalStep::Type => return Shell::TerminalTyping,
    }
    Shell::Nothing
}

fn run_due(runner: &mut Runner, due: Due<Later>) -> (Shell, bool) {
    match due {
        Due::Select(column, row) => {
            crate::ffi::nr_column_select(column, row);
            (Shell::Nothing, true)
        }
        Due::Type { words, .. } => {
            let typed = format!("{words}\r");
            // SAFETY: `typed` is readable for its length.
            unsafe {
                crate::ffi::nr_terminal_input(
                    crate::ffi::front_terminal(),
                    typed.as_ptr(),
                    typed.len(),
                )
            };
            (Shell::Nothing, false)
        }
        Due::Trace => (Shell::Trace, false),
        Due::Window => (
            script::get()
                .picture_path("window.png")
                .map_or(Shell::Nothing, Shell::Window),
            false,
        ),
        Due::Step(step) => {
            let sync = step.redraws();
            (run_step(runner, step), sync)
        }
        Due::Later(later) => match later {
            Later::Look => (Shell::LookAt(runner.looking), false),
            Later::Shot { path, alone } => (Shell::Shot { path, alone }, false),
            Later::SaveMenus(stem) => (Shell::SaveMenus(stem), false),
            Later::ReportMenus { tag, stem } => (Shell::ReportMenus { tag, stem }, false),
            Later::WaitForUpdate => {
                wait_for_update(runner);
                (Shell::Nothing, false)
            }
            Later::Quit => (Shell::Quit, false),
        },
    }
}

/// The step due at `now` on the shell's monotonic clock (seconds), and when to ask again. Call
/// it once the window is open, then as each step's delay says.
#[no_mangle]
pub extern "C" fn nr_script_next(now: f64) -> NrScriptStep {
    let mut runner = runner();
    runner.now = now;
    match runner.plan.next(now) {
        Next::Over => {
            // Reported for the check runner, which waits on this instead of its own timer, since a
            // busy app runs steps later than their half-second gaps.
            let elapsed = runner
                .plan
                .elapsed(now)
                .expect("the plan begins on its first call");
            eprintln!("script\tover after {elapsed:.1} s");
            hand(&mut runner, Shell::Nothing, false, 0.0, true)
        }
        Next::Wait(seconds) => hand(&mut runner, Shell::Nothing, false, seconds, false),
        Next::Now(due) => {
            let (shell, sync) = run_due(&mut runner, due);
            // Ask again at once; anything else due now runs on the next turn.
            hand(&mut runner, shell, sync, 0.0, false)
        }
    }
}

/// Receives the centre of the row or card an `at:` step named, in points from the window's
/// top-left, or `found` false when no visible row or card has that name. Returns the event to
/// perform there (numbered as in `nr_script_next`; nothing else is due in it).
#[no_mangle]
pub extern "C" fn nr_script_at(found: bool, x: f64, y: f64) -> NrScriptStep {
    let mut runner = runner();
    let Some((event, name)) = runner.at.take() else {
        return hand(&mut runner, Shell::Nothing, false, 0.0, false);
    };
    if !found {
        eprintln!("at\tno row or card named {event}:{name}");
        return hand(&mut runner, Shell::Nothing, false, 0.0, false);
    }
    let words = format!("{event}:{}:{}", x.round() as i64, y.round() as i64);
    let shell = Event::parse(&words).map_or(Shell::Nothing, |event| event_step(&mut runner, event));
    hand(&mut runner, shell, false, 0.0, false)
}

/// Receives the scroll state a look asked for, of the columns, the board or the scroller under
/// the last point: `offset` is the clip's left (top for the board and scroller), `shown` its
/// width (height), `extent` the total columns width (content height), plus whether the window
/// is visible and uncovered. Once it settles, the line is reported and the steps continue.
#[no_mangle]
pub extern "C" fn nr_script_looked(offset: f64, shown: f64, extent: f64, visible: bool) {
    let mut runner = runner();
    let seen = match runner.looking {
        Scrolled::Columns => script::columns_line(offset, shown, extent),
        Scrolled::Board => script::board_line(crate::ffi::board_zoom(), offset, shown, extent),
        Scrolled::Under => script::scrolled_line(offset, shown, extent),
    };
    match runner.settle.look(seen, visible) {
        Some(line) => {
            eprintln!("{line}");
            runner.plan.hold(false);
        }
        None => {
            let now = runner.now;
            runner
                .plan
                .after(now, script::SETTLE_LOOK, Due::Later(Later::Look));
        }
    }
}

/// One press from `NR_TERM_TYPE`: the platform key code, modifiers (`NR_HELD_*`), the text it
/// types, and the text it types unmodified.
#[repr(C)]
pub struct NrScriptPress {
    pub code: u16,
    pub modifiers: u32,
    pub characters: NrText,
    pub plain: NrText,
}

/// Fills `out` with press `index` of `NR_TERM_TYPE`; false past the last. Its text lives for
/// the whole process.
///
/// # Safety
/// `out` must point to writable storage for one `NrScriptPress`.
#[no_mangle]
pub unsafe extern "C" fn nr_script_press(index: usize, out: *mut NrScriptPress) -> bool {
    let Some(press) = script::get().terminal.presses.get(index) else {
        return false;
    };
    // SAFETY: the caller passes storage for one press.
    unsafe {
        out.write(NrScriptPress {
            code: press.code,
            modifiers: press.held,
            characters: NrText::borrowing(&press.characters),
            plain: NrText::borrowing(&press.plain),
        });
    }
    true
}

// MARK: Window state for checks

fn window_state() -> script::Window {
    use crate::ffi::*;
    let tabs = (0..nr_tabs_count())
        .map(|index| {
            (
                owned(nr_tab_title(index)),
                with_columns(|columns| columns.tab_kind(index)).unwrap_or(0),
            )
        })
        .collect();
    // SAFETY: storage for one file; its title is copied at once.
    let mut file = unsafe { std::mem::zeroed::<NrOpened>() };
    let opened = unsafe { nr_opened(&mut file) }.then(|| owned(file.title));
    let (mut header, sort) = with_columns(|columns| {
        let header: Vec<String> = columns
            .list_columns()
            .iter()
            .map(|column| column.label().to_owned())
            .collect();
        (header, columns.list_sort())
    })
    .unwrap_or_default();
    if let Some(sort) = sort {
        header.push(format!(
            "sort {} {}",
            sort.key.label(),
            if sort.ascending {
                "ascending"
            } else {
                "descending"
            }
        ));
    }
    let focus = crate::board::ffi::nr_board_focus();
    let cards: Vec<String> = crate::app()
        .board_rows
        .relock()
        .iter()
        .map(|entry| entry.title.clone())
        .collect();
    let card = |index: i64| {
        usize::try_from(index)
            .ok()
            .and_then(|index| cards.get(index).cloned())
    };
    script::Window {
        view: folder_view(),
        tabs,
        active: nr_tabs_active(),
        back: nr_history_can_go_back(),
        forward: nr_history_can_go_forward(),
        settings_tab: nr_settings_tab(),
        opened,
        fills: with_columns(|columns| columns.opened_fills_tab()).unwrap_or(false),
        opened_key: owned(crate::window::ffi::nr_opened_identity()),
        header,
        card_selected: card(focus.selected),
        card_focused: card(focus.focused),
        cards,
    }
}

/// # Safety
/// `probe` must point to a NUL-terminated string.
unsafe fn probe_of(probe: *const c_char) -> script::Probe {
    // SAFETY: the caller passes a NUL-terminated string.
    let text = unsafe { text_of(probe) };
    serde_json::from_str(&text).expect("the shell's probe is the core's `script::Probe`")
}

/// Reports, as one line of JSON, what a person would see and act on (`ev:observe`): core state
/// merged with view-only state from `probe`, a JSON `script::Probe`.
///
/// # Safety
/// `probe` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_script_observe(probe: *const c_char) {
    // SAFETY: as documented.
    let probe = unsafe { probe_of(probe) };
    eprintln!("{}", script::observe_line(&window_state(), &probe));
}

/// Reports what the window shows in one line (`ev:state`), including view-only state.
///
/// # Safety
/// `probe` must point to a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn nr_script_state(probe: *const c_char) {
    // SAFETY: as documented.
    let probe = unsafe { probe_of(probe) };
    eprintln!("{}", script::state_line(&window_state(), &probe));
}

/// An image: `width` × `height` RGB pixels in row order.
#[repr(C)]
pub struct NrScriptPicture {
    pub width: usize,
    pub height: usize,
    pub bytes: *const u8,
}

/// The preview frame the core last wrote for a check, to composite into a window screenshot,
/// since video goes straight to its own layer, which a view screenshot misses. False when there
/// is none. The bytes are valid until the next call.
///
/// # Safety
/// `out` must point to writable storage for one `NrScriptPicture`.
#[no_mangle]
pub unsafe extern "C" fn nr_script_preview_picture(out: *mut NrScriptPicture) -> bool {
    static PIXELS: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    let Some(data) = script::get()
        .picture_path("preview.ppm")
        .and_then(|path| std::fs::read(path).ok())
    else {
        return false;
    };
    let Some((width, height, pixels)) = script::read_ppm(&data) else {
        return false;
    };
    let mut kept = PIXELS.relock();
    *kept = pixels.to_vec();
    // SAFETY: the caller passes storage for one picture.
    unsafe {
        out.write(NrScriptPicture {
            width,
            height,
            bytes: kept.as_ptr(),
        })
    };
    true
}

// MARK: Terminal keys and wheel

/// A character key pressed in the terminal with Control or Option held: `layout` is the keyboard
/// layout id, `typed` what the key typed, `plain` what it types with Shift alone, `modifiers`
/// the held keys (`menus::COMMAND`…). True when it was sent as a key
/// (`terminal::input::held_character`); false when it is text for the input system.
///
/// # Safety
/// `layout`, `typed` and `plain` must point to NUL-terminated strings.
#[no_mangle]
pub unsafe extern "C" fn nr_terminal_held_key(
    number: u64,
    layout: *const c_char,
    typed: *const c_char,
    plain: *const c_char,
    modifiers: u32,
) -> bool {
    // SAFETY: the caller passes NUL-terminated strings.
    let (layout, typed, plain) = unsafe { (text_of(layout), text_of(typed), text_of(plain)) };
    let Some((key, told)) =
        crate::terminal::input::held_character(&layout, &typed, &plain, Modifiers::held(modifiers))
    else {
        return false;
    };
    let crate::terminal::input::Key::Character(character) = key else {
        return false;
    };
    crate::ffi::with_shown(number, |terminal| {
        terminal.key(crate::terminal::input::Key::Character(character), told)
    });
    true
}

/// The wheel or trackpad over the terminal at `x`, `y` (pixels): `delta` is in view points when
/// `precise` (a trackpad), else in wheel notches; `scale` is pixels per point.
#[no_mangle]
pub extern "C" fn nr_terminal_wheel(
    number: u64,
    delta: f64,
    precise: bool,
    scale: f64,
    x: f32,
    y: f32,
    modifiers: u32,
) {
    crate::ffi::terminal_scroll(
        number,
        crate::terminal::input::wheel_travel(delta, precise, scale),
        precise,
        x,
        y,
        Modifiers::held(modifiers),
    );
}
