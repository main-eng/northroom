//! A check's script: what an unattended run does and when, read once from its environment.
//! `NR_SELECT=column:row,…` selects rows 1.5 s apart (`NR_TYPE` and `NR_TYPE2` are typed into
//! the terminal a selection opens), then `NR_DO=step,…` runs its steps 0.5 s apart, each after
//! the previous one finished. `NR_TRACE` reports the window layout after 1 s, and
//! `NR_SNAPSHOT_DIR` saves a window screenshot after `NR_SNAPSHOT_AFTER` seconds (default 2).
//!
//! Every step is parsed here. Steps that only call the core run in the core (`script::ffi`); the
//! shell gets only those that need the platform: synthetic user events, screenshots, menus and
//! views only it can inspect. An empty variable counts as unset, as everywhere in `launch`.

pub mod ffi;

use crate::columns::View;
use crate::ffi::NrKey;
use crate::terminal::input::Key;
use std::collections::{BTreeMap, VecDeque};

/// Modifier bits as steps name them (`cmd+shift`), numbered as everywhere in the FFI.
pub use crate::menus::{COMMAND, CONTROL, OPTION, SHIFT};

/// A key's FFI code. A character is `Other`, with its text passed alongside.
pub fn key_code(key: Key) -> NrKey {
    key.boundary()
}

fn modifiers(names: Option<&str>) -> u32 {
    names.unwrap_or("").split('+').fold(0, |held, name| {
        held | match name {
            "cmd" => COMMAND,
            "shift" => SHIFT,
            "alt" => OPTION,
            "ctrl" => CONTROL,
            _ => 0,
        }
    })
}

/// A key named in a step: `return`, `escape`, an arrow, …, or a single character (the first).
fn named_key(name: &str) -> Option<Key> {
    Some(match name {
        "return" => Key::Return,
        "escape" => Key::Escape,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "space" => Key::Character(' '),
        // Steps are comma-separated, so a comma needs a name.
        "comma" => Key::Character(','),
        "tab" => Key::Tab,
        "delete" => Key::Backspace,
        "home" => Key::Home,
        "end" => Key::End,
        "fdelete" => Key::Delete,
        "f2" => Key::Function(2),
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        _ => Key::Character(name.chars().next()?),
    })
}

/// A key named in a player step, as in `named_key`, with `space` for the space bar.
fn named_player_key(name: &str) -> Option<NrKey> {
    Some(match named_key(name)? {
        Key::Character(' ') => NrKey::Space,
        key => key.boundary(),
    })
}

/// A point in the window, from its top-left corner, y down.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// The kind of wheel step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wheel {
    /// One precise (trackpad-like, pixel) step with no phase.
    Precise,
    /// One click of a mouse wheel (line units).
    Lines,
    /// A whole two-finger gesture of the same travel, with its phases and momentum.
    Swipe,
}

/// A synthetic event a step makes (`ev:`), or a state report the shell produces for it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// Moves the pointer there, dragging while the button is down.
    Move(Point),
    Down(Point, u32),
    Up(Point),
    Click(Point, u32),
    DoubleClick(Point, u32),
    RightClick(Point),
    MiddleClick(Point),
    Scroll {
        at: Point,
        down: f64,
        across: f64,
        held: u32,
        wheel: Wheel,
    },
    /// A whole two-finger pinch at `at` that spreads the fingers by `by` (pinches in when
    /// negative), spread over the gesture's events.
    Pinch {
        at: Point,
        by: f64,
    },
    /// The columns' horizontal scroll, once settled (`Settle`).
    Columns,
    /// The board's column count and scroll, once settled.
    Board,
    /// The vertical scroll of the scroller under the last point, once settled.
    Scrolled,
    /// The tab strip's scroll and each tab's width.
    Tabs,
    /// The window state the behaviour check reads, as JSON (`observe`).
    Observe,
    /// The view, tabs, history, folder, selection, what is open and what has focus (`state`).
    State,
    /// A key down and up. `characters` is what a character key types, shift applied.
    Key {
        key: Key,
        held: u32,
        characters: String,
    },
    /// Each character typed in turn (`_` stands for a space).
    Text(String),
    /// `name.png`: the window with all the app's other windows composited in place, or the window
    /// alone.
    Shot {
        name: String,
        alone: bool,
    },
    /// Simulates a system drag from the row at `from` to `to` without involving the system.
    DragCheck {
        from: Point,
        to: Point,
    },
    /// The file at `path` dropped at `at` from another app.
    Drop {
        at: Point,
        path: String,
    },
    /// Which window and view is under the last point, and what has focus.
    Say,
    /// Nothing; the next step runs half a second later.
    Wait,
}

impl Event {
    /// Parses the text after `ev:`. `None` for a step that makes no event (an unknown name, or one
    /// missing arguments).
    pub fn parse(words: &str) -> Option<Self> {
        let parts: Vec<&str> = words.split(':').collect();
        let number = |index: usize| parts.get(index).and_then(|part| part.parse::<f64>().ok());
        let point = || {
            Some(Point {
                x: number(1)?,
                y: number(2)?,
            })
        };
        let held = |index: usize| modifiers(parts.get(index).copied());
        Some(match parts[0] {
            "move" => Self::Move(point()?),
            "down" => Self::Down(point()?, held(3)),
            "up" => Self::Up(point()?),
            "click" => Self::Click(point()?, held(3)),
            "dclick" => Self::DoubleClick(point()?, held(3)),
            "rclick" => Self::RightClick(point()?),
            "mclick" => Self::MiddleClick(point()?),
            name @ ("scroll" | "wheel" | "swipe") => Self::Scroll {
                at: point()?,
                down: number(3)?,
                across: number(4).unwrap_or(0.0),
                held: held(5),
                wheel: match name {
                    "wheel" => Wheel::Lines,
                    "swipe" => Wheel::Swipe,
                    _ => Wheel::Precise,
                },
            },
            "pinch" => Self::Pinch {
                at: point()?,
                by: number(3)?,
            },
            "columns" => Self::Columns,
            "board" => Self::Board,
            "scrolled" => Self::Scrolled,
            "tabs" => Self::Tabs,
            "observe" => Self::Observe,
            "state" => Self::State,
            "key" => {
                let held = held(2);
                let key = named_key(parts.get(1)?)?;
                let characters = match key {
                    Key::Character(character) if held & SHIFT != 0 => {
                        character.to_uppercase().collect()
                    }
                    Key::Character(character) => character.to_string(),
                    _ => String::new(),
                };
                Self::Key {
                    key,
                    held,
                    characters,
                }
            }
            "text" if parts.len() >= 2 => Self::Text(parts[1..].join(":").replace('_', " ")),
            "shot" => Self::Shot {
                name: (*parts.get(1)?).to_owned(),
                alone: parts.get(2) == Some(&"window"),
            },
            "dragcheck" => Self::DragCheck {
                from: point()?,
                to: Point {
                    x: number(3)?,
                    y: number(4)?,
                },
            },
            "drop" if parts.len() >= 4 => Self::Drop {
                at: point()?,
                path: parts[3..].join(":"),
            },
            "say" => Self::Say,
            "wait" => Self::Wait,
            _ => return None,
        })
    }
}

/// A step of the file preview (`preview…`).
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewStep {
    Play,
    Mute,
    /// Presses the scrub bar at its middle.
    Half,
    /// Drags the scrub bar from a quarter to three quarters, then releases.
    Drag,
    /// Reports what the preview shows and has sent to the sound output, as one line.
    Stats,
    /// Reports the open page's players, as one line of JSON (from the shell).
    Page,
    /// Opens the list's first row as a double click does, which records its use.
    Use,
    /// `previewrate1.5`: plays at that rate.
    Rate(f64),
    /// `previewkeyleft`: presses the named key (`space` for the space bar).
    Key(NrKey),
    Nothing,
}

impl PreviewStep {
    fn parse(name: &str) -> Self {
        match name {
            "previewplay" => Self::Play,
            "previewmute" => Self::Mute,
            "previewhalf" => Self::Half,
            "previewdrag" => Self::Drag,
            "previewstats" => Self::Stats,
            "previewpage" => Self::Page,
            "previewuse" => Self::Use,
            _ => {
                if let Some(rate) = name.strip_prefix("previewrate") {
                    rate.parse().map_or(Self::Nothing, Self::Rate)
                } else if let Some(key) = name.strip_prefix("previewkey") {
                    named_player_key(key).map_or(Self::Nothing, Self::Key)
                } else {
                    Self::Nothing
                }
            }
        }
    }
}

/// A columns step (`col/`), after which each column's selection is reported.
#[derive(Clone, Debug, PartialEq)]
pub enum ColumnsStep {
    /// Focuses the column, as a click in it does.
    Focus(Option<usize>),
    /// Selects rows `first` to `last` together, as ⇧ and ⌘ clicks do.
    Group(Option<usize>, Option<(usize, usize)>),
    /// A key pressed in the column.
    Key(Option<usize>, Option<(Key, u32)>),
    /// Only reports the selections.
    Say,
}

impl ColumnsStep {
    fn parse(words: &str) -> Self {
        let parts: Vec<&str> = words.split(':').filter(|part| !part.is_empty()).collect();
        let column = parts.get(1).and_then(|part| part.parse().ok());
        match parts.first().copied() {
            Some("focus") => Self::Focus(column),
            Some("group") => {
                let ends: Vec<usize> = parts
                    .get(2)
                    .unwrap_or(&"")
                    .split('-')
                    .filter_map(|end| end.parse().ok())
                    .collect();
                Self::Group(
                    column,
                    ends.first()
                        .zip(ends.last())
                        .map(|(first, last)| (*first, *last)),
                )
            }
            Some("key") => Self::Key(
                column,
                match parts.get(2).copied() {
                    Some("left") => Some((Key::Left, 0)),
                    Some("right") => Some((Key::Right, 0)),
                    Some("up") => Some((Key::Up, 0)),
                    Some("down") => Some((Key::Down, 0)),
                    Some("escape") => Some((Key::Escape, 0)),
                    Some("shiftdown") => Some((Key::Down, SHIFT)),
                    Some("cmdup") => Some((Key::Up, COMMAND)),
                    _ => None,
                },
            ),
            _ => Self::Say,
        }
    }
}

/// A terminal step (`term…`), each configured by its `NR_TERM_*` variable.
#[derive(Clone, Debug, PartialEq)]
pub enum TerminalStep {
    Focus(bool),
    /// `NR_TERM_DRAG=x0,y0,x1,y1[,clicks]` in the terminal's pixels.
    Drag,
    /// Writes what a copy would take to `copied.txt` beside the screenshots.
    Copy,
    /// `NR_TERM_PASTE`, `\n` for a line break.
    Paste,
    /// `NR_TERM_REFER=terminal|file|…`: rows of the first column, by title, passed to the first
    /// named terminal row as if dropped on it.
    Refer,
    /// `NR_TERM_SCROLL=pixels`, sent as trackpad scrolling.
    Scroll,
    Compose,
    /// `NR_TERM_TYPE=keyCode:modifiers:characters:plain;…`: keys pressed on the terminal view
    /// through its key handling and the input system (done by the shell).
    Type,
}

/// A parsed step.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Back,
    Forward,
    NewTab,
    CloseTab,
    FirstTab,
    /// Opens Settings (`settings`).
    Settings,
    /// Reports the menu bar's titles, one menu per line.
    MenuBar,
    /// Reports every menu bar shortcut.
    MenuKeys,
    /// The page's block gutter on row 1: its menu, or its "+".
    Gutter {
        plus: bool,
    },
    /// The ink appearance.
    Ink,
    /// Sets the app's appearance as the system would (`look:light`, `look:dark`; `None` for
    /// `look:system`, to follow the system again), and the app follows it like a system change.
    Look(Option<bool>),
    /// Reports the effective theme from the core (`theme`).
    Theme,
    /// Expands the first two folders in the list.
    Expand,
    /// Opens and saves the first card's menu and its Tags submenu.
    CardMenu,
    /// Opens and saves the folder background menu.
    BackgroundMenu,
    /// Opens the naming dialog and leaves it open: Rename for the first file of the shown column,
    /// as a board asks for a name.
    Dialog,
    /// Connect Phone: the pairing dialog with its QR code.
    Phone,
    /// Shows the folder as a board, a list or columns.
    View(View),
    /// Clicks a list header.
    Sort(u32),
    /// Toggles list columns, then optionally sets one column's width.
    ListColumns(&'static [u32], Option<(u32, f64)>),
    EditFirst,
    BoldWord,
    PasteBlocks,
    TableRow,
    Caption,
    /// A page media block step (`mediafocus`, `mediadelete`, …, `editcaption`).
    Media(String),
    Tick,
    Retype,
    Logs,
    OpenLogs,
    /// Check for Updates… (`true`), or a scheduled check.
    UpdateCheck(bool),
    /// Quits once a downloaded update is ready, so install-on-quit can be checked.
    UpdateQuitWhenReady,
    /// Sets the title bar's update button state for a screenshot.
    UpdateStatus(String),
    TrayMenu,
    /// Uses a settings control: action and value.
    Act(String, String),
    Event(Event),
    /// An event at the centre of the named row or card, wherever it is drawn now: the event name
    /// (`click`, `dclick`, …) and the row name.
    At(String, String),
    /// `at:` with no name.
    AtNothing(String),
    HtmlProbe,
    /// A right click at that point, reporting where the menu opened.
    RightClick(Point),
    /// `col/…`: the columns step and its text.
    Columns(ColumnsStep, String),
    /// Rests the pointer on row n of the last open menu.
    MenuRow(i64),
    /// `new:folder`, `new:page`, `new:terminal`: runs the background menu's New entry in the shown
    /// column, as a person would.
    New(String),
    Preview(PreviewStep),
    Terminal(TerminalStep),
    /// Only redraws the window: `update:launched` (the app's launch update check runs meanwhile)
    /// and any unknown step.
    Nothing,
}

/// An `act:` step's action and value, split at the first `=` (empty value if none).
fn action_and_value(rest: &str) -> (String, String) {
    let (action, value) = rest.split_once('=').unwrap_or((rest, ""));
    (action.to_owned(), value.to_owned())
}

/// Parses a prefixed step from the text after the prefix and the whole step text.
type ReadStep = fn(&str, &str) -> Step;

/// Prefixed steps in the order they are tried: each prefix and its parser.
const PREFIXED_STEPS: &[(&str, ReadStep)] = &[
    ("update:status=", |status, _| {
        Step::UpdateStatus(status.to_owned())
    }),
    ("act:", |rest, _| {
        let (action, value) = action_and_value(rest);
        Step::Act(action, value)
    }),
    // A step that makes no event still leaves the window unchanged.
    ("ev:", |rest, _| {
        Step::Event(Event::parse(rest).unwrap_or(Event::Wait))
    }),
    ("at:", |rest, _| Step::at(rest)),
    ("rightclick:", |rest, _| Step::right_click(rest)),
    ("col/", |rest, text| {
        Step::Columns(ColumnsStep::parse(rest), text.to_owned())
    }),
    ("menurow:", |row, _| Step::MenuRow(row.parse().unwrap_or(0))),
    ("new:", |kind, _| Step::New(format!("new-{kind}"))),
    ("preview", |_, text| Step::Preview(PreviewStep::parse(text))),
];

impl Step {
    pub fn parse(text: &str) -> Self {
        if let Some(step) = Self::named(text) {
            return step;
        }
        PREFIXED_STEPS
            .iter()
            .find_map(|(prefix, read)| text.strip_prefix(prefix).map(|rest| read(rest, text)))
            .unwrap_or(Self::Nothing)
    }

    /// The step named by a whole word, if any.
    fn named(text: &str) -> Option<Self> {
        Some(match text {
            "back" => Self::Back,
            "forward" => Self::Forward,
            "tab" => Self::NewTab,
            "close" => Self::CloseTab,
            "first" => Self::FirstTab,
            "settings" => Self::Settings,
            "menubar" => Self::MenuBar,
            "menukeys" => Self::MenuKeys,
            "blockmenu" => Self::Gutter { plus: false },
            "blockplus" => Self::Gutter { plus: true },
            "ink" => Self::Ink,
            "look:light" => Self::Look(Some(false)),
            "look:dark" => Self::Look(Some(true)),
            "look:system" => Self::Look(None),
            "theme" => Self::Theme,
            "expand" => Self::Expand,
            "menu" => Self::CardMenu,
            "backgroundmenu" => Self::BackgroundMenu,
            "dialog" => Self::Dialog,
            "phone" => Self::Phone,
            "board" => Self::View(View::Board),
            "list" => Self::View(View::List),
            "columns" => Self::View(View::Columns),
            "sortname" => Self::Sort(0),
            "sortsize" => Self::Sort(3),
            // Date Added, Dimensions and Duration: columns filled only by the file index.
            "indexcolumns" => Self::ListColumns(&[5, 13, 14], None),
            // Date Created, Extension and Permissions join the list; Kind is dragged narrow.
            "morecolumns" => Self::ListColumns(&[2, 9, 12], Some((4, 70.0))),
            "editfirst" => Self::EditFirst,
            "boldword" => Self::BoldWord,
            "pasteblocks" => Self::PasteBlocks,
            "tablerow" => Self::TableRow,
            "caption" => Self::Caption,
            "mediafocus" | "mediadelete" | "mediareturn" | "mediaup" | "mediadown"
            | "editcaption" => Self::Media(text.to_owned()),
            "logs" => Self::Logs,
            "openlogs" => Self::OpenLogs,
            "update:check" => Self::UpdateCheck(true),
            "update:background" => Self::UpdateCheck(false),
            // The app runs the launch update check itself; the step only waits for it.
            "update:launched" => Self::Nothing,
            "update:quitwhenready" => Self::UpdateQuitWhenReady,
            "traymenu" => Self::TrayMenu,
            "htmlprobe" => Self::HtmlProbe,
            "tick" => Self::Tick,
            "retype" => Self::Retype,
            "termfocus" => Self::Terminal(TerminalStep::Focus(true)),
            "termblur" => Self::Terminal(TerminalStep::Focus(false)),
            "termdrag" => Self::Terminal(TerminalStep::Drag),
            "termcopy" => Self::Terminal(TerminalStep::Copy),
            "termpaste" => Self::Terminal(TerminalStep::Paste),
            "termrefer" => Self::Terminal(TerminalStep::Refer),
            "termscroll" => Self::Terminal(TerminalStep::Scroll),
            "termcompose" => Self::Terminal(TerminalStep::Compose),
            "termtype" => Self::Terminal(TerminalStep::Type),
            _ => return None,
        })
    }

    /// `at:<event>:<name>`'s step, from what follows `at:`.
    fn at(rest: &str) -> Self {
        let parts: Vec<&str> = rest
            .splitn(2, ':')
            .filter(|part| !part.is_empty())
            .collect();
        match parts[..] {
            [event, name] => Self::At(event.to_owned(), name.to_owned()),
            _ => Self::AtNothing(rest.to_owned()),
        }
    }

    /// `rightclick:<x>:<y>`'s step, from what follows `rightclick:`.
    fn right_click(rest: &str) -> Self {
        let at: Vec<f64> = rest
            .split(':')
            .filter_map(|part| part.parse().ok())
            .collect();
        match at[..] {
            [x, y] => Self::RightClick(Point { x, y }),
            _ => Self::Nothing,
        }
    }

    /// Whether the window redraws after the step runs. Events, columns steps and the HTML probe
    /// leave the window unchanged.
    pub fn redraws(&self) -> bool {
        !matches!(
            self,
            Self::Event(_)
                | Self::At(..)
                | Self::AtNothing(_)
                | Self::HtmlProbe
                | Self::Columns(..)
        )
    }
}

/// One press of `NR_TERM_TYPE`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    /// The platform key code.
    pub code: u16,
    /// Modifier bits as the FFI numbers them (`menus::COMMAND`…).
    pub held: u32,
    pub characters: String,
    /// What the key types unmodified.
    pub plain: String,
}

fn presses(text: &str) -> Vec<Press> {
    text.split(';')
        .filter(|press| !press.is_empty())
        .filter_map(|press| {
            let parts: Vec<&str> = press.split(':').collect();
            let [code, held, characters, plain] = parts[..] else {
                return None;
            };
            Some(Press {
                code: code.parse().ok()?,
                held: held.parse().ok()?,
                characters: characters.to_owned(),
                plain: plain.to_owned(),
            })
        })
        .collect()
}

/// Inputs for terminal steps (`NR_TERM_*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerminalSettings {
    pub drag: Vec<f32>,
    pub paste: String,
    pub refer: Vec<String>,
    pub scroll: Vec<f32>,
    pub compose: String,
    pub presses: Vec<Press>,
}

/// The script a run was started with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Script {
    /// `NR_SELECT`, one entry per selection. An entry without a column and row selects nothing but
    /// still takes its time slot.
    pub selections: Vec<Option<(usize, usize)>>,
    pub steps: Vec<Step>,
    /// `NR_TYPE`, then `NR_TYPE2` 2 s later, each followed by Return.
    pub typed: Option<String>,
    pub typed_again: Option<String>,
    pub trace: bool,
    /// `NR_SNAPSHOT_DIR`: where screenshots and step output go.
    pub pictures: Option<String>,
    pub picture_after: f64,
    /// `NR_BLOCK` and `NR_TEXT`: the page block a step acts on and the text it sets.
    pub block: usize,
    pub text: String,
    pub terminal: TerminalSettings,
}

impl Script {
    pub fn read(variable: impl Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| variable(name).filter(|value| !value.is_empty());
        let numbers = |name: &str| -> Vec<f32> {
            set(name)
                .unwrap_or_default()
                .split(',')
                .filter_map(|part| part.parse().ok())
                .collect()
        };
        Self {
            selections: set("NR_SELECT")
                .unwrap_or_default()
                .split(',')
                .filter(|step| !step.is_empty())
                .map(|step| {
                    let parts: Vec<usize> = step
                        .split(':')
                        .filter_map(|part| part.parse().ok())
                        .collect();
                    match parts[..] {
                        [column, row] => Some((column, row)),
                        _ => None,
                    }
                })
                .collect(),
            steps: set("NR_DO")
                .unwrap_or_default()
                .split(',')
                .filter(|step| !step.is_empty())
                .map(Step::parse)
                .collect(),
            typed: set("NR_TYPE"),
            typed_again: set("NR_TYPE2"),
            trace: set("NR_TRACE").is_some(),
            pictures: set("NR_SNAPSHOT_DIR"),
            picture_after: set("NR_SNAPSHOT_AFTER")
                .and_then(|after| after.parse().ok())
                .unwrap_or(2.0),
            block: set("NR_BLOCK")
                .and_then(|block| block.parse().ok())
                .unwrap_or(0),
            text: set("NR_TEXT").unwrap_or_default(),
            terminal: TerminalSettings {
                drag: numbers("NR_TERM_DRAG"),
                paste: set("NR_TERM_PASTE")
                    .unwrap_or_default()
                    .replace("\\n", "\n"),
                refer: set("NR_TERM_REFER")
                    .unwrap_or_default()
                    .split('|')
                    .filter(|title| !title.is_empty())
                    .map(str::to_owned)
                    .collect(),
                scroll: numbers("NR_TERM_SCROLL"),
                compose: set("NR_TERM_COMPOSE").unwrap_or_default(),
                presses: presses(&set("NR_TERM_TYPE").unwrap_or_default()),
            },
        }
    }

    /// The path for a file a step writes, beside the screenshots; `None` when the run saves none.
    pub fn picture_path(&self, name: &str) -> Option<String> {
        self.pictures
            .as_ref()
            .map(|directory| format!("{directory}/{name}"))
    }
}

/// This process's script.
pub fn get() -> &'static Script {
    static SCRIPT: std::sync::OnceLock<Script> = std::sync::OnceLock::new();
    SCRIPT.get_or_init(|| Script::read(|name| std::env::var(name).ok()))
}

// MARK: Timing

/// The gap between selections.
pub const SELECTION_GAP: f64 = 1.5;
/// Delay from the window opening to the first selection.
pub const FIRST_SELECTION: f64 = 0.2;
/// Delay from the last selection's slot to the first step.
pub const FIRST_STEP: f64 = 0.4;
/// Gap between steps, counted from when the previous one ran.
pub const STEP_GAP: f64 = 0.5;
/// How often a held step is rechecked.
pub const HELD_LOOK: f64 = 0.05;
/// Delay of `NR_TYPE` after its selection, and of `NR_TYPE2` after `NR_TYPE`.
pub const TYPE_AFTER: f64 = 1.0;
pub const TYPE_AGAIN_AFTER: f64 = 2.0;
/// Delay after the window opens before reporting its layout (`NR_TRACE`).
pub const TRACE_AFTER: f64 = 1.0;

/// A scheduled item.
#[derive(Clone, Debug, PartialEq)]
pub enum Due<T> {
    Select(usize, usize),
    /// Text typed into the terminal, then Return: `NR_TYPE` (`first`), or `NR_TYPE2`.
    Type {
        words: String,
        first: bool,
    },
    Trace,
    /// The window screenshot (`window.png`).
    Window,
    Step(Step),
    /// Work a step deferred.
    Later(T),
}

/// The script's timeline on the shell's monotonic clock (seconds): the selections, the steps in
/// order, and deferred work. `T` is the deferred work type.
pub struct Plan<T> {
    began: Option<f64>,
    script_selections: Vec<Option<(usize, usize)>>,
    timed: Vec<(f64, Due<T>)>,
    steps: VecDeque<Step>,
    step_due: f64,
    /// A step is still working after it ran, so later steps wait (`hold`).
    holding: bool,
    typed: Option<String>,
    typed_again: Option<String>,
    trace: bool,
    window_after: Option<f64>,
}

/// The result of `Plan::next`.
#[derive(Debug, PartialEq)]
pub enum Next<T> {
    /// Due now.
    Now(Due<T>),
    /// Nothing for this many seconds.
    Wait(f64),
    /// The script is finished.
    Over,
}

impl<T> Plan<T> {
    pub fn new(script: &Script) -> Self {
        Self {
            began: None,
            script_selections: script.selections.clone(),
            timed: Vec::new(),
            steps: script.steps.iter().cloned().collect(),
            step_due: 0.0,
            holding: false,
            typed: script.typed.clone(),
            typed_again: script.typed_again.clone(),
            trace: script.trace,
            window_after: script.pictures.as_ref().map(|_| script.picture_after),
        }
    }

    /// Starts the timeline at `now`, when the window has just opened.
    fn begin(&mut self, now: f64) {
        self.began = Some(now);
        for (index, selection) in std::mem::take(&mut self.script_selections)
            .into_iter()
            .enumerate()
        {
            if let Some((column, row)) = selection {
                self.after(
                    now,
                    FIRST_SELECTION + index as f64 * SELECTION_GAP,
                    Due::Select(column, row),
                );
            }
        }
        if self.trace {
            self.after(now, TRACE_AFTER, Due::Trace);
        }
        if let Some(after) = self.window_after {
            self.after(now, after, Due::Window);
        }
    }

    /// Schedules `due` at `now + seconds`, after anything already due then.
    pub fn after(&mut self, now: f64, seconds: f64, due: Due<T>) {
        let at = now + seconds;
        let place = self
            .timed
            .iter()
            .position(|(when, _)| *when > at)
            .unwrap_or(self.timed.len());
        self.timed.insert(place, (at, due));
    }

    /// Seconds from the window opening to `now`; `None` before it opened.
    pub fn elapsed(&self, now: f64) -> Option<f64> {
        self.began.map(|began| now - began)
    }

    /// Makes later steps wait (`true`) or continue.
    pub fn hold(&mut self, holding: bool) {
        self.holding = holding;
    }

    /// What is due at `now`, or how long until something is.
    pub fn next(&mut self, now: f64) -> Next<T> {
        if self.began.is_none() {
            return self.start_clock(now);
        }
        loop {
            let Some(soonest) = self.soonest() else {
                return Next::Over;
            };
            if soonest > now {
                return Next::Wait(soonest - now);
            }
            // Earliest first; on a tie, a timed item runs before a step.
            if self.first_timed() == Some(soonest) {
                return self.run_first_timed(now);
            }
            if self.holding {
                self.step_due = now + HELD_LOOK;
                continue;
            }
            let step = self.steps.pop_front().expect("a step is due");
            self.step_due = now + STEP_GAP;
            return Next::Now(Due::Step(step));
        }
    }

    /// The first call only starts the clock; nothing runs before the window has finished opening.
    fn start_clock(&mut self, now: f64) -> Next<T> {
        let selections = self.script_selections.len();
        self.begin(now);
        self.step_due = now + FIRST_STEP + selections as f64 * SELECTION_GAP;
        match self.soonest() {
            None => Next::Over,
            Some(soonest) => Next::Wait((soonest - now).max(0.0)),
        }
    }

    /// When the first timed item is due.
    fn first_timed(&self) -> Option<f64> {
        self.timed.first().map(|(when, _)| *when)
    }

    /// When the next timed item or step is due; `None` once nothing is left.
    fn soonest(&self) -> Option<f64> {
        let step = (!self.steps.is_empty()).then_some(self.step_due);
        self.first_timed().into_iter().chain(step).reduce(f64::min)
    }

    /// Runs the first timed item, scheduling the typing that follows a selection (`NR_TYPE`) or the
    /// first typing (`NR_TYPE2`).
    fn run_first_timed(&mut self, now: f64) -> Next<T> {
        let (_, due) = self.timed.remove(0);
        let follows = match &due {
            Due::Select(..) => self.typed.clone().map(|words| (TYPE_AFTER, words, true)),
            Due::Type { first: true, .. } => self
                .typed_again
                .clone()
                .map(|words| (TYPE_AGAIN_AFTER, words, false)),
            _ => None,
        };
        if let Some((after, words, first)) = follows {
            self.after(now, after, Due::Type { words, first });
        }
        Next::Now(due)
    }
}

// MARK: Scroll settling

/// Reads the columns' scroll until it is unchanged for three reads after the first, up to
/// thirty reads, since a scroller may move on the display's next refresh after a wheel event.
#[derive(Default)]
pub struct Settle {
    last: Option<String>,
    same: u32,
    looks: u32,
}

/// Interval between reads.
pub const SETTLE_LOOK: f64 = 0.1;

impl Settle {
    /// Takes one more read; `Some` with the line to report once settled.
    pub fn look(&mut self, seen: String, visible: bool) -> Option<String> {
        let last = self.last.replace(seen.clone())?;
        self.same = if seen == last { self.same + 1 } else { 0 };
        self.looks += 1;
        (self.same >= 3 || self.looks >= 30).then(|| {
            // A fully covered window or a sleeping display gets no refresh, so its report does not
            // count.
            if visible {
                seen
            } else {
                format!("{seen}\tcovered")
            }
        })
    }
}

/// The columns' horizontal scroll and range: `offset` is the clip's left, `shown` its width,
/// `width` the total columns width.
pub fn columns_line(offset: f64, shown: f64, width: f64) -> String {
    format!(
        "columns\toffset {} of {}\twidth {}",
        offset.round() as i64,
        (width - shown).max(0.0).round() as i64,
        width as i64
    )
}

/// The board's column count and vertical scroll: `offset` is the clip's top, `shown` its
/// height, `height` the board's.
pub fn board_line(columns: usize, offset: f64, shown: f64, height: f64) -> String {
    format!(
        "board\tcolumns {columns}\toffset {} of {}\theight {}",
        offset.round() as i64,
        (height - shown).max(0.0).round() as i64,
        height.round() as i64
    )
}

/// The vertical scroll and range of the scroller under the last point: `offset` from its top,
/// `shown` its height, `height` its content height.
pub fn scrolled_line(offset: f64, shown: f64, height: f64) -> String {
    format!(
        "scrolled\toffset {} of {}\theight {}",
        offset.round() as i64,
        (height - shown).max(0.0).round() as i64,
        height.round() as i64
    )
}

/// The effective theme (`theme`): light or dark, its background as `#rrggbb`, and the choice in
/// Settings (`light`, `dark` or `system`).
pub fn theme_line(dark: bool, background: [u8; 3], choice: &str) -> String {
    let [red, green, blue] = background;
    format!(
        "theme\t{}\tbackground #{red:02x}{green:02x}{blue:02x}\tchoice {choice}",
        if dark { "dark" } else { "light" }
    )
}

// MARK: Window state for checks

/// Window state only the platform views know: which folder view is drawn, the rows the list or
/// columns draw and select, open menus and dialogs, what has focus, and the open file's view.
#[derive(serde::Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct Probe {
    /// Settings is shown in place of the folder.
    pub settings: bool,
    /// The folder view drawn.
    pub folder: View,
    /// The list's rows, or the last column's.
    pub rows: Vec<String>,
    /// What the list selects, or each column's selected rows (joined with commas).
    pub selected: Vec<String>,
    pub expanded: Vec<String>,
    pub renaming: bool,
    /// How many columns are drawn.
    pub columns: usize,
    pub menus: Vec<Vec<String>>,
    pub dialog: String,
    pub keys: String,
    /// The view the open file is drawn in, and whether it sits beside the columns.
    pub file: String,
    pub beside: bool,
    /// For `state`: the list or columns selection as one line, the window's child windows, and
    /// what has focus.
    pub said: String,
    pub over: String,
}

/// The core's part of the window state.
pub struct Window {
    pub view: View,
    pub tabs: Vec<(String, u32)>,
    pub active: usize,
    pub back: bool,
    pub forward: bool,
    pub settings_tab: i32,
    /// The open file's title, and whether it fills the tab.
    pub opened: Option<String>,
    pub fills: bool,
    pub opened_key: String,
    /// The list's columns' labels and how it is sorted (`sort <label> ascending`).
    pub header: Vec<String>,
    /// The board's cards, the selected one and the focused one.
    pub cards: Vec<String>,
    pub card_selected: Option<String>,
    pub card_focused: Option<String>,
}

/// What a person would see and act on, as one line of JSON for a check driving the app
/// (`ev:observe`).
pub fn observe_line(window: &Window, probe: &Probe) -> String {
    use serde_json::Value;
    let texts = |list: &[String]| Value::from(list.to_vec());
    let (mut selected, mut expanded, mut shown, mut header) = match probe.folder {
        View::List => (
            probe.selected.clone(),
            probe.expanded.clone(),
            probe.rows.clone(),
            window.header.clone(),
        ),
        View::Board => (
            window.card_selected.iter().cloned().collect(),
            Vec::new(),
            window.cards.clone(),
            Vec::new(),
        ),
        View::Columns => (
            probe
                .selected
                .iter()
                .filter(|titles| !titles.is_empty())
                .cloned()
                .collect(),
            Vec::new(),
            probe.rows.clone(),
            vec![format!("{} columns", probe.columns)],
        ),
    };
    let renaming = probe.folder != View::Board && probe.renaming;
    if window.fills {
        // The folder view is hidden behind the file.
        (selected, expanded, shown, header) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    }
    let view = if probe.settings {
        "settings"
    } else {
        window.view.name()
    };
    let tabs: Vec<String> = window.tabs.iter().map(|(title, _)| title.clone()).collect();
    let state: BTreeMap<&str, Value> = BTreeMap::from([
        ("view", view.into()),
        ("tabs", texts(&tabs)),
        ("active", window.active.into()),
        ("back", window.back.into()),
        ("forward", window.forward.into()),
        (
            "opened",
            window
                .opened
                .clone()
                .filter(|_| window.fills)
                .unwrap_or_default()
                .into(),
        ),
        ("selected", texts(&selected)),
        ("expanded", texts(&expanded)),
        ("renaming", renaming.into()),
        ("rows", texts(&shown)),
        ("header", texts(&header)),
        ("menus", Value::from(probe.menus.clone())),
        ("dialog", probe.dialog.clone().into()),
        // An open drawn menu has keyboard focus.
        (
            "keys",
            if probe.menus.is_empty() {
                probe.keys.clone()
            } else {
                "menu".to_owned()
            }
            .into(),
        ),
        (
            "card",
            window
                .card_focused
                .clone()
                .filter(|_| probe.folder == View::Board)
                .unwrap_or_default()
                .into(),
        ),
        ("file", probe.file.clone().into()),
        ("beside", probe.beside.into()),
    ]);
    format!(
        "observe\t{}",
        serde_json::to_string(&state).expect("plain data")
    )
}

/// What the window shows, in one line (`ev:state`): the view, tabs, history, folder, selection,
/// what is open over it, and what has focus.
pub fn state_line(window: &Window, probe: &Probe) -> String {
    let tabs: Vec<String> = window
        .tabs
        .iter()
        .map(|(title, kind)| format!("{title}:{kind}"))
        .collect();
    let selected = match probe.folder {
        View::Board => format!("board {}", window.card_selected.clone().unwrap_or_default()),
        _ => probe.said.clone(),
    };
    format!(
        "state\tview {}\ttabs {}/{} [{}]\thistory {}{}\tsettings {}\topened {} fills {}\tselected {{{}}}\tover [{}]\tkeys {}",
        window.view.number(),
        window.active,
        window.tabs.len(),
        tabs.join("|"),
        if window.back { "b" } else { "-" },
        if window.forward { "f" } else { "-" },
        window.settings_tab,
        if window.opened_key.is_empty() {
            "-"
        } else {
            &window.opened_key
        },
        window.fills,
        selected,
        probe.over,
        probe.keys
    )
}

// MARK: Preview reports

/// What the preview shows and has sent to the sound output (`previewstats`).
pub struct PreviewStats<'a> {
    pub clock: &'a str,
    pub playing: bool,
    pub ended: bool,
    pub position: f64,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub audio: bool,
    pub muted: bool,
    pub volume: f32,
    pub rate: f64,
    pub fullscreen: bool,
    pub frames: u64,
    pub missing: u64,
    pub resyncs: u32,
    pub eased: u64,
    pub peak: f32,
    pub at: f64,
    pub drift: f64,
    pub largest: f64,
}

pub fn preview_line(stats: &PreviewStats) -> String {
    let flag = u8::from;
    format!(
        "preview: {} playing={} ended={} position={:.3} duration={:.3} {}x{} audio={} muted={} volume={:.2} rate={:.2} fullscreen={} \
         | sound: frames={} missing={} resyncs={} eased={} peak={:.3} at={:.3} drift={:+.4} largest={:+.4}",
        stats.clock,
        flag(stats.playing),
        flag(stats.ended),
        stats.position,
        stats.duration,
        stats.width,
        stats.height,
        flag(stats.audio),
        flag(stats.muted),
        f64::from(stats.volume),
        stats.rate,
        flag(stats.fullscreen),
        stats.frames,
        stats.missing,
        stats.resyncs,
        stats.eased,
        f64::from(stats.peak),
        stats.at,
        stats.drift,
        stats.largest
    )
}

/// Parses the image the core wrote for a check (`preview.ppm`): "P6", width, height and max
/// value, each followed by whitespace, then three bytes per pixel.
pub fn read_ppm(data: &[u8]) -> Option<(usize, usize, &[u8])> {
    let mut fields = Vec::new();
    let mut index = 0;
    while fields.len() < 4 && index < data.len() {
        let start = index;
        while index < data.len() && !data[index].is_ascii_whitespace() {
            index += 1;
        }
        if index > start {
            fields.push(std::str::from_utf8(&data[start..index]).ok()?);
        }
        index += 1;
    }
    let [kind, width, height, _] = fields[..] else {
        return None;
    };
    let (width, height): (usize, usize) = (width.parse().ok()?, height.parse().ok()?);
    let length = width * height * 3;
    (kind == "P6" && width > 0 && height > 0 && data.len().saturating_sub(index) >= length)
        .then(|| (width, height, &data[index..index + length]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(pairs: &[(&str, &str)]) -> Script {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Script::read(move |name| {
            pairs
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, value)| value.clone())
        })
    }

    fn point(x: f64, y: f64) -> Point {
        Point { x, y }
    }

    /// Every step form the app's scripted runs use (including update-check.sh and the website's
    /// app screenshots), each with its expected meaning.
    #[test]
    fn every_step_the_checks_use_parses() {
        let cases: Vec<(&str, Step)> = vec![
            ("board", Step::View(View::Board)),
            ("list", Step::View(View::List)),
            ("columns", Step::View(View::Columns)),
            ("settings", Step::Settings),
            ("tab", Step::NewTab),
            ("menubar", Step::MenuBar),
            ("htmlprobe", Step::HtmlProbe),
            ("update:check", Step::UpdateCheck(true)),
            ("update:quitwhenready", Step::UpdateQuitWhenReady),
            ("update:launched", Step::Nothing),
            (
                "update:status=available",
                Step::UpdateStatus("available".into()),
            ),
            ("update:status=ready", Step::UpdateStatus("ready".into())),
            (
                "update:status=downloading",
                Step::UpdateStatus("downloading".into()),
            ),
            (
                "col/focus:0",
                Step::Columns(ColumnsStep::Focus(Some(0)), "col/focus:0".into()),
            ),
            (
                "col/focus:2",
                Step::Columns(ColumnsStep::Focus(Some(2)), "col/focus:2".into()),
            ),
            (
                "col/group:0:7-9",
                Step::Columns(
                    ColumnsStep::Group(Some(0), Some((7, 9))),
                    "col/group:0:7-9".into(),
                ),
            ),
            ("at:dclick:Kinds", Step::At("dclick".into(), "Kinds".into())),
            (
                "at:dclick:Media players",
                Step::At("dclick".into(), "Media players".into()),
            ),
            (
                "at:click:notes.md",
                Step::At("click".into(), "notes.md".into()),
            ),
            ("previewpage", Step::Preview(PreviewStep::Page)),
            ("previewstats", Step::Preview(PreviewStep::Stats)),
            ("ev:wait", Step::Event(Event::Wait)),
            ("ev:observe", Step::Event(Event::Observe)),
            ("ev:tabs", Step::Event(Event::Tabs)),
            ("ev:columns", Step::Event(Event::Columns)),
            ("ev:board", Step::Event(Event::Board)),
            ("ev:scrolled", Step::Event(Event::Scrolled)),
            ("look:light", Step::Look(Some(false))),
            ("look:dark", Step::Look(Some(true))),
            ("look:system", Step::Look(None)),
            ("theme", Step::Theme),
            (
                "ev:wheel:640:400:1:0:cmd",
                Step::Event(Event::Scroll {
                    at: point(640.0, 400.0),
                    down: 1.0,
                    across: 0.0,
                    held: COMMAND,
                    wheel: Wheel::Lines,
                }),
            ),
            (
                "ev:wheel:420:250:-1",
                Step::Event(Event::Scroll {
                    at: point(420.0, 250.0),
                    down: -1.0,
                    across: 0.0,
                    held: 0,
                    wheel: Wheel::Lines,
                }),
            ),
            (
                "ev:click:640:421",
                Step::Event(Event::Click(point(640.0, 421.0), 0)),
            ),
            (
                "ev:click:500:300:cmd",
                Step::Event(Event::Click(point(500.0, 300.0), COMMAND)),
            ),
            (
                "ev:click:500:300:shift",
                Step::Event(Event::Click(point(500.0, 300.0), SHIFT)),
            ),
            (
                "ev:click:500:300:cmd+shift",
                Step::Event(Event::Click(point(500.0, 300.0), COMMAND | SHIFT)),
            ),
            (
                "ev:dclick:460:60",
                Step::Event(Event::DoubleClick(point(460.0, 60.0), 0)),
            ),
            (
                "ev:rclick:900:600",
                Step::Event(Event::RightClick(point(900.0, 600.0))),
            ),
            (
                "ev:mclick:120:20",
                Step::Event(Event::MiddleClick(point(120.0, 20.0))),
            ),
            (
                "ev:move:640:795",
                Step::Event(Event::Move(point(640.0, 795.0))),
            ),
            (
                "ev:down:63:180",
                Step::Event(Event::Down(point(63.0, 180.0), 0)),
            ),
            ("ev:up:340:600", Step::Event(Event::Up(point(340.0, 600.0)))),
            (
                "ev:scroll:400:20:0:-1000",
                Step::Event(Event::Scroll {
                    at: point(400.0, 20.0),
                    down: 0.0,
                    across: -1000.0,
                    held: 0,
                    wheel: Wheel::Precise,
                }),
            ),
            (
                "ev:scroll:300:400:120:0",
                Step::Event(Event::Scroll {
                    at: point(300.0, 400.0),
                    down: 120.0,
                    across: 0.0,
                    held: 0,
                    wheel: Wheel::Precise,
                }),
            ),
            (
                "ev:scroll:640:500:400",
                Step::Event(Event::Scroll {
                    at: point(640.0, 500.0),
                    down: 400.0,
                    across: 0.0,
                    held: 0,
                    wheel: Wheel::Precise,
                }),
            ),
            (
                "ev:key:down",
                Step::Event(Event::Key {
                    key: Key::Down,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:up",
                Step::Event(Event::Key {
                    key: Key::Up,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:left",
                Step::Event(Event::Key {
                    key: Key::Left,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:right",
                Step::Event(Event::Key {
                    key: Key::Right,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:down:shift",
                Step::Event(Event::Key {
                    key: Key::Down,
                    held: SHIFT,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:right:shift",
                Step::Event(Event::Key {
                    key: Key::Right,
                    held: SHIFT,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:return",
                Step::Event(Event::Key {
                    key: Key::Return,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:escape",
                Step::Event(Event::Key {
                    key: Key::Escape,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:delete",
                Step::Event(Event::Key {
                    key: Key::Backspace,
                    held: 0,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:delete:cmd",
                Step::Event(Event::Key {
                    key: Key::Backspace,
                    held: COMMAND,
                    characters: String::new(),
                }),
            ),
            (
                "ev:key:t",
                Step::Event(Event::Key {
                    key: Key::Character('t'),
                    held: 0,
                    characters: "t".into(),
                }),
            ),
            (
                "ev:key:k:cmd",
                Step::Event(Event::Key {
                    key: Key::Character('k'),
                    held: COMMAND,
                    characters: "k".into(),
                }),
            ),
            (
                "ev:key:comma:cmd",
                Step::Event(Event::Key {
                    key: Key::Character(','),
                    held: COMMAND,
                    characters: ",".into(),
                }),
            ),
            ("ev:text:x", Step::Event(Event::Text("x".into()))),
            ("ev:text:Blue", Step::Event(Event::Text("Blue".into()))),
            (
                "ev:drop:400:300:/tmp/a b.png",
                Step::Event(Event::Drop {
                    at: Point { x: 400.0, y: 300.0 },
                    path: "/tmp/a b.png".into(),
                }),
            ),
            (
                "ev:shot:board-click",
                Step::Event(Event::Shot {
                    name: "board-click".into(),
                    alone: false,
                }),
            ),
            (
                "ev:shot:list-drag-over-file:window",
                Step::Event(Event::Shot {
                    name: "list-drag-over-file".into(),
                    alone: true,
                }),
            ),
        ];
        for (text, expected) in cases {
            assert_eq!(Step::parse(text), expected, "{text}");
        }
    }

    #[test]
    fn the_rest_of_the_grammar() {
        assert_eq!(
            Step::parse("ev:key:.:cmd+shift"),
            Step::Event(Event::Key {
                key: Key::Character('.'),
                held: COMMAND | SHIFT,
                characters: ".".into()
            })
        );
        assert_eq!(
            Step::parse("ev:key:a:shift"),
            Step::Event(Event::Key {
                key: Key::Character('a'),
                held: SHIFT,
                characters: "A".into()
            })
        );
        assert_eq!(
            Step::parse("ev:key:space"),
            Step::Event(Event::Key {
                key: Key::Character(' '),
                held: 0,
                characters: " ".into()
            })
        );
        assert_eq!(
            Step::parse("ev:key:f2"),
            Step::Event(Event::Key {
                key: Key::Function(2),
                held: 0,
                characters: String::new()
            })
        );
        assert_eq!(
            Step::parse("ev:key:fdelete"),
            Step::Event(Event::Key {
                key: Key::Delete,
                held: 0,
                characters: String::new()
            })
        );
        assert_eq!(
            Step::parse("ev:key:up:alt+ctrl"),
            Step::Event(Event::Key {
                key: Key::Up,
                held: OPTION | CONTROL,
                characters: String::new()
            })
        );
        // Text after the first colon is typed too, and `_` is a space.
        assert_eq!(
            Step::parse("ev:text:a_b:c"),
            Step::Event(Event::Text("a b:c".into()))
        );
        assert_eq!(
            Step::parse("ev:wheel:10:20:3:4:shift"),
            Step::Event(Event::Scroll {
                at: point(10.0, 20.0),
                down: 3.0,
                across: 4.0,
                held: SHIFT,
                wheel: Wheel::Lines
            })
        );
        assert_eq!(
            Step::parse("ev:swipe:10:20:90"),
            Step::Event(Event::Scroll {
                at: point(10.0, 20.0),
                down: 90.0,
                across: 0.0,
                held: 0,
                wheel: Wheel::Swipe
            })
        );
        assert_eq!(
            Step::parse("ev:pinch:10:20:-0.5"),
            Step::Event(Event::Pinch {
                at: point(10.0, 20.0),
                by: -0.5
            })
        );
        assert_eq!(
            Step::parse("ev:dragcheck:1:2:3:4"),
            Step::Event(Event::DragCheck {
                from: point(1.0, 2.0),
                to: point(3.0, 4.0)
            })
        );
        assert_eq!(Step::parse("ev:say"), Step::Event(Event::Say));
        assert_eq!(Step::parse("ev:state"), Step::Event(Event::State));
        // A step missing arguments makes no event.
        for text in [
            "ev:click:1",
            "ev:scroll:1:2",
            "ev:pinch:1:2",
            "ev:key",
            "ev:key:",
            "ev:text",
            "ev:dragcheck:1:2:3",
            "ev:",
            "ev:freeze",
        ] {
            assert_eq!(Step::parse(text), Step::Event(Event::Wait), "{text}");
        }
        assert_eq!(Step::parse("at:click:"), Step::AtNothing("click:".into()));
        assert_eq!(
            Step::parse("rightclick:100:200"),
            Step::RightClick(point(100.0, 200.0))
        );
        assert_eq!(Step::parse("rightclick:100"), Step::Nothing);
        assert_eq!(Step::parse("menurow:2"), Step::MenuRow(2));
        assert_eq!(Step::parse("new:folder"), Step::New("new-folder".into()));
        assert_eq!(Step::parse("menurow:x"), Step::MenuRow(0));
        assert_eq!(
            Step::parse("act:name=Ada"),
            Step::Act("name".into(), "Ada".into())
        );
        assert_eq!(
            Step::parse("act:save"),
            Step::Act("save".into(), String::new())
        );
        assert_eq!(Step::parse("blockplus"), Step::Gutter { plus: true });
        assert_eq!(
            Step::parse("mediadelete"),
            Step::Media("mediadelete".into())
        );
        assert_eq!(
            Step::parse("morecolumns"),
            Step::ListColumns(&[2, 9, 12], Some((4, 70.0)))
        );
        assert_eq!(Step::parse("update:background"), Step::UpdateCheck(false));
        assert_eq!(Step::parse("termtype"), Step::Terminal(TerminalStep::Type));
        assert_eq!(
            Step::parse("termblur"),
            Step::Terminal(TerminalStep::Focus(false))
        );
        assert_eq!(
            Step::parse("previewrate1.5"),
            Step::Preview(PreviewStep::Rate(1.5))
        );
        assert_eq!(
            Step::parse("previewkeyright"),
            Step::Preview(PreviewStep::Key(NrKey::Right))
        );
        assert_eq!(
            Step::parse("previewkeyspace"),
            Step::Preview(PreviewStep::Key(NrKey::Space))
        );
        assert_eq!(
            Step::parse("previewnothing"),
            Step::Preview(PreviewStep::Nothing)
        );
        assert_eq!(Step::parse("whatever"), Step::Nothing);
        assert_eq!(
            Step::parse("col/key:0:shiftdown"),
            Step::Columns(
                ColumnsStep::Key(Some(0), Some((Key::Down, SHIFT))),
                "col/key:0:shiftdown".into()
            )
        );
        assert_eq!(
            Step::parse("col/state"),
            Step::Columns(ColumnsStep::Say, "col/state".into())
        );
    }

    #[test]
    fn redrawing_follows_the_step() {
        assert!(Step::parse("board").redraws());
        assert!(Step::parse("whatever").redraws());
        assert!(Step::parse("rightclick:1").redraws());
        assert!(Step::parse("update:launched").redraws());
        for text in [
            "ev:wait",
            "ev:bogus",
            "at:click:x",
            "at:",
            "htmlprobe",
            "col/focus:0",
        ] {
            assert!(!Step::parse(text).redraws(), "{text}");
        }
    }

    #[test]
    fn a_script_reads_its_variables() {
        assert_eq!(
            script(&[]),
            Script {
                picture_after: 2.0,
                ..Script::default()
            }
        );
        let read = script(&[
            ("NR_SELECT", "0:2,1:0,x,,3"),
            ("NR_DO", "board,,ev:wait"),
            ("NR_TYPE", "ls"),
            ("NR_SNAPSHOT_DIR", "/tmp/shots"),
            ("NR_SNAPSHOT_AFTER", "7.5"),
            ("NR_BLOCK", "3"),
            ("NR_TEXT", "words"),
            ("NR_TERM_DRAG", "1,2,3,4,2"),
            ("NR_TERM_PASTE", "a\\nb"),
            ("NR_TERM_REFER", "term|a.png||b.png"),
            ("NR_TERM_TYPE", "0:4:\u{3}:c;bad;12:2:œ:q"),
        ]);
        assert_eq!(read.selections, [Some((0, 2)), Some((1, 0)), None, None]);
        assert_eq!(
            read.steps,
            [Step::View(View::Board), Step::Event(Event::Wait)]
        );
        assert_eq!(read.typed.as_deref(), Some("ls"));
        assert_eq!(
            read.picture_path("window.png").as_deref(),
            Some("/tmp/shots/window.png")
        );
        assert_eq!(
            (read.picture_after, read.block, read.text.as_str()),
            (7.5, 3, "words")
        );
        assert_eq!(read.terminal.drag, [1.0, 2.0, 3.0, 4.0, 2.0]);
        assert_eq!(read.terminal.paste, "a\nb");
        assert_eq!(read.terminal.refer, ["term", "a.png", "b.png"]);
        assert_eq!(
            read.terminal.presses,
            [
                Press {
                    code: 0,
                    held: 4,
                    characters: "\u{3}".into(),
                    plain: "c".into()
                },
                Press {
                    code: 12,
                    held: 2,
                    characters: "œ".into(),
                    plain: "q".into()
                },
            ]
        );
        // An empty value counts as unset.
        assert_eq!(
            script(&[("NR_SNAPSHOT_DIR", ""), ("NR_TRACE", "")]).pictures,
            None
        );
        assert!(script(&[("NR_TRACE", "1")]).trace);
    }

    /// Runs a plan, completing each due item at once, and lists what ran when.
    fn timeline(script: &Script, until: f64) -> Vec<(f64, Due<()>)> {
        let mut plan: Plan<()> = Plan::new(script);
        let mut now = 0.0;
        let mut seen = Vec::new();
        while now <= until {
            match plan.next(now) {
                Next::Now(due) => seen.push(((now * 1000.0_f64).round() / 1000.0, due)),
                Next::Wait(seconds) => now += seconds,
                Next::Over => break,
            }
        }
        seen
    }

    #[test]
    fn selections_then_steps_half_a_second_apart() {
        let script = script(&[
            ("NR_SELECT", "0:1,bad,2:3"),
            ("NR_DO", "board,list"),
            ("NR_SNAPSHOT_DIR", "/x"),
            ("NR_SNAPSHOT_AFTER", "5"),
        ]);
        assert_eq!(
            timeline(&script, 10.0),
            [
                (0.2, Due::Select(0, 1)),
                (3.2, Due::Select(2, 3)),
                // After every selection's turn, the bad one's too: 0.4 + 3 × 1.5.
                (4.9, Due::Step(Step::View(View::Board))),
                (5.0, Due::Window),
                (5.4, Due::Step(Step::View(View::List))),
            ]
        );
    }

    #[test]
    fn typing_follows_each_selection() {
        let script = script(&[
            ("NR_SELECT", "0:0"),
            ("NR_TYPE", "ls"),
            ("NR_TYPE2", "pwd"),
            ("NR_TRACE", "1"),
        ]);
        let seen = timeline(&script, 10.0);
        assert_eq!(seen[0], (0.2, Due::Select(0, 0)));
        assert_eq!(seen[1], (1.0, Due::Trace));
        assert_eq!(
            seen[2],
            (
                1.2,
                Due::Type {
                    words: "ls".into(),
                    first: true
                }
            )
        );
        assert_eq!(
            seen[3],
            (
                3.2,
                Due::Type {
                    words: "pwd".into(),
                    first: false
                }
            )
        );
        assert_eq!(seen.len(), 4);
    }

    #[test]
    fn a_held_step_keeps_the_rest_waiting() {
        let script = script(&[("NR_DO", "board,list")]);
        let mut plan: Plan<&str> = Plan::new(&script);
        assert_eq!(plan.elapsed(0.0), None, "the window has not opened");
        assert_eq!(plan.next(0.0), Next::Wait(0.4));
        assert_eq!(
            plan.next(0.4),
            Next::Now(Due::Step(Step::View(View::Board)))
        );
        plan.hold(true);
        plan.after(0.4, 0.1, Due::Later("look"));
        assert_eq!(plan.next(0.5), Next::Now(Due::Later("look")));
        // The next step was due at 0.9; while held, it is rechecked every 50 ms.
        assert!(matches!(plan.next(0.9), Next::Wait(wait) if (wait - HELD_LOOK).abs() < 1e-9));
        plan.hold(false);
        assert_eq!(
            plan.next(0.96),
            Next::Now(Due::Step(Step::View(View::List)))
        );
        assert_eq!(plan.next(1.0), Next::Over);
        assert_eq!(plan.elapsed(1.0), Some(1.0));
    }

    #[test]
    fn the_columns_settle_after_three_equal_looks() {
        let mut settle = Settle::default();
        assert_eq!(
            settle.look("a".into(), true),
            None,
            "the first look is the one to compare with"
        );
        assert_eq!(settle.look("b".into(), true), None);
        assert_eq!(settle.look("b".into(), true), None);
        assert_eq!(settle.look("b".into(), true), None);
        assert_eq!(settle.look("b".into(), false), Some("b\tcovered".into()));
        let mut moving = Settle::default();
        moving.look("0".into(), true);
        let said: Vec<Option<String>> = (1..=30)
            .map(|look| moving.look(look.to_string(), true))
            .collect();
        assert_eq!(
            said.last(),
            Some(&Some("30".into())),
            "at most thirty looks"
        );
        assert!(said[..29].iter().all(Option::is_none));
        assert_eq!(
            columns_line(319.6, 960.0, 1279.4),
            "columns\toffset 320 of 319\twidth 1279"
        );
        assert_eq!(
            columns_line(0.0, 1200.0, 800.0),
            "columns\toffset 0 of 0\twidth 800"
        );
        assert_eq!(
            board_line(2, 79.6, 736.0, 1500.4),
            "board\tcolumns 2\toffset 80 of 764\theight 1500"
        );
        assert_eq!(
            board_line(6, 0.0, 736.0, 736.0),
            "board\tcolumns 6\toffset 0 of 0\theight 736"
        );
        assert_eq!(
            scrolled_line(80.4, 728.0, 1906.6),
            "scrolled\toffset 80 of 1179\theight 1907"
        );
        assert_eq!(
            scrolled_line(0.0, 320.0, 200.0),
            "scrolled\toffset 0 of 0\theight 200"
        );
    }

    #[test]
    fn the_theme_line_says_the_look_its_background_and_the_choice() {
        assert_eq!(
            theme_line(true, [17, 17, 17], "system"),
            "theme\tdark\tbackground #111111\tchoice system"
        );
        assert_eq!(
            theme_line(false, [250, 250, 251], "light"),
            "theme\tlight\tbackground #fafafb\tchoice light"
        );
    }

    fn window() -> Window {
        Window {
            view: View::List,
            tabs: vec![("workspace".into(), 0), ("Notes".into(), 0)],
            active: 1,
            back: true,
            forward: false,
            settings_tab: -1,
            opened: Some("tone.wav".into()),
            fills: false,
            opened_key: "3:1:tone.wav:false".into(),
            header: vec!["Name".into(), "Kind".into(), "sort Name ascending".into()],
            cards: vec!["Assets".into(), "tone.wav".into()],
            card_selected: Some("tone.wav".into()),
            card_focused: Some("Assets".into()),
        }
    }

    #[test]
    fn observe_merges_the_views_probe() {
        let probe = Probe {
            folder: View::List,
            rows: vec!["Assets".into(), "tone.wav".into()],
            selected: vec!["tone.wav".into()],
            expanded: vec!["Assets".into()],
            keys: "ListTableView".into(),
            ..Probe::default()
        };
        assert_eq!(
            observe_line(&window(), &probe),
            "observe\t{\"active\":1,\"back\":true,\"beside\":false,\"card\":\"\",\"dialog\":\"\",\"expanded\":[\"Assets\"],\
             \"file\":\"\",\"forward\":false,\"header\":[\"Name\",\"Kind\",\"sort Name ascending\"],\"keys\":\"ListTableView\",\
             \"menus\":[],\"opened\":\"\",\"renaming\":false,\"rows\":[\"Assets\",\"tone.wav\"],\"selected\":[\"tone.wav\"],\
             \"tabs\":[\"workspace\",\"Notes\"],\"view\":\"list\"}"
        );
        // The core supplies a board's cards; an open menu has focus; a file filling the tab hides
        // the folder.
        let board = Probe {
            folder: View::Board,
            menus: vec![vec!["Close tab".into()]],
            keys: "BoardCanvas".into(),
            ..Probe::default()
        };
        let seen: serde_json::Value =
            serde_json::from_str(observe_line(&window(), &board).split_once('\t').unwrap().1)
                .unwrap();
        assert_eq!(seen["rows"], serde_json::json!(["Assets", "tone.wav"]));
        assert_eq!(seen["selected"], serde_json::json!(["tone.wav"]));
        assert_eq!(
            (
                seen["card"].as_str(),
                seen["keys"].as_str(),
                seen["header"].as_array().map(Vec::len)
            ),
            (Some("Assets"), Some("menu"), Some(0))
        );
        let filled = Window {
            fills: true,
            ..window()
        };
        let columns = Probe {
            folder: View::Columns,
            columns: 3,
            selected: vec!["a".into(), String::new()],
            rows: vec!["x".into()],
            settings: true,
            ..Probe::default()
        };
        let seen: serde_json::Value =
            serde_json::from_str(observe_line(&filled, &columns).split_once('\t').unwrap().1)
                .unwrap();
        assert_eq!(
            (
                seen["opened"].as_str(),
                seen["view"].as_str(),
                seen["rows"].as_array().map(Vec::len)
            ),
            (Some("tone.wav"), Some("settings"), Some(0))
        );
        let seen: serde_json::Value = serde_json::from_str(
            observe_line(&window(), &columns)
                .split_once('\t')
                .unwrap()
                .1,
        )
        .unwrap();
        assert_eq!(
            (seen["header"].clone(), seen["selected"].clone()),
            (serde_json::json!(["3 columns"]), serde_json::json!(["a"]))
        );
    }

    #[test]
    fn the_state_line() {
        let probe = Probe {
            folder: View::List,
            said: "list 2 rows; selected tone.wav; open ".into(),
            over: "".into(),
            keys: "ListTableView".into(),
            ..Probe::default()
        };
        assert_eq!(
            state_line(&window(), &probe),
            "state\tview 1\ttabs 1/2 [workspace:0|Notes:0]\thistory b-\tsettings -1\topened 3:1:tone.wav:false fills false\
             \tselected {list 2 rows; selected tone.wav; open }\tover []\tkeys ListTableView"
        );
        let board = Probe {
            folder: View::Board,
            keys: "none".into(),
            ..Probe::default()
        };
        let line = state_line(
            &Window {
                opened_key: String::new(),
                ..window()
            },
            &board,
        );
        assert!(
            line.contains("\topened - fills false\tselected {board tone.wav}\t"),
            "{line}"
        );
    }

    #[test]
    fn the_previews_line() {
        let stats = PreviewStats {
            clock: "0:01 / 0:10",
            playing: true,
            ended: false,
            position: 1.23456,
            duration: 10.0,
            width: 1280,
            height: 720,
            audio: true,
            muted: false,
            volume: 1.0,
            rate: 1.5,
            fullscreen: false,
            frames: 48000,
            missing: 0,
            resyncs: 2,
            eased: 7,
            peak: 0.5,
            at: 1.0,
            drift: -0.00123,
            largest: 0.0,
        };
        assert_eq!(
            preview_line(&stats),
            "preview: 0:01 / 0:10 playing=1 ended=0 position=1.235 duration=10.000 1280x720 audio=1 muted=0 volume=1.00 rate=1.50 \
             fullscreen=0 | sound: frames=48000 missing=0 resyncs=2 eased=7 peak=0.500 at=1.000 drift=-0.0012 largest=+0.0000"
        );
    }

    #[test]
    fn the_previews_picture_is_read() {
        let mut data = b"P6\n2 1\n255\n".to_vec();
        data.extend_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(read_ppm(&data), Some((2, 1, &[1, 2, 3, 4, 5, 6][..])));
        assert_eq!(read_ppm(&data[..data.len() - 1]), None, "short");
        assert_eq!(read_ppm(b"P5\n1 1\n255\n\0"), None);
    }
}
