//! The terminal: a shell process, its emulation, and the frame the shell draws (ADR-0001). One
//! terminal serves every platform; the shell draws its frame with the rest of the window and
//! forwards keys, the mouse and the grid's size.
//!
//! The emulator and the process run behind `engine`, the one module that knows the emulator
//! library. This module owns the session: what the person did to it, what it shows, and when
//! the shell should draw it again (`watch_frames`).

pub mod activity;
pub mod agent;
pub mod cells;
mod engine;
pub mod exit;
pub mod frame;
pub mod harness;
pub mod input;
pub mod launch;
pub mod palette;
pub mod resume;
pub(crate) mod screen;
pub mod transcript;

use crate::terminal::cells::{GridSize, Modes, Rgb, Scroll};
use crate::terminal::engine::Engine;
use crate::terminal::frame::{CellMetrics, Frame};
use crate::terminal::input::{
    cell_at, encode_key, encode_mouse, paste_bytes, wheel_as_arrows, Key, Modifiers, MouseAction,
    Wheel,
};
use crate::terminal::launch::Launch;
use crate::Relock;
pub use palette::{Palette, Queried};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a blinking cursor stays shown, then hidden.
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(600);
/// A notch of a mouse wheel moves this many lines.
const WHEEL_NOTCH_LINES: f32 = 3.0;
/// The fewest columns and lines a grid has, however small its view.
const MIN_COLUMNS: usize = 2;
const MIN_LINES: usize = 1;

/// The view a terminal is drawn in: its size in pixels and the cell the shell measured from its
/// font (`frame::CellMetrics::of_font`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewSize {
    pub width: u32,
    pub height: u32,
    pub cell: CellMetrics,
}

/// The grid as the process is told it: whole cells, and each cell's pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TextArea {
    grid: GridSize,
    cell: (f32, f32),
}

/// The grid that fills `view` in whole cells.
fn grid_size(view: ViewSize) -> GridSize {
    GridSize {
        columns: ((view.width as f32 / view.cell.width).floor() as usize).max(MIN_COLUMNS),
        lines: ((view.height as f32 / view.cell.height).floor() as usize).max(MIN_LINES),
    }
}

/// The callback the shell registered for terminals with something new to draw
/// (`watch_frames`).
static FRAME_CHANGED: OnceLock<extern "C" fn()> = OnceLock::new();

/// Registers `changed` to be called, from any thread, when a terminal on screen has something
/// new to draw, so the shell draws its window again. It is called once until the shell takes
/// the terminal's next frame (`Terminal::frame`). Call once at launch.
pub fn watch_frames(changed: extern "C" fn()) {
    let _ = FRAME_CHANGED.set(changed);
}

/// The longest typed-ahead input waits for the shell's prompt before it is sent anyway, for a
/// shell that never turns its echo off.
#[cfg(unix)]
const TYPED_AHEAD_MOST: Duration = Duration::from_secs(5);

struct State {
    /// The session is on screen; it keeps running while it is not.
    shown: bool,
    /// The session's activity, read from what the program reports about itself.
    session: crate::terminal::activity::Session,
    /// The shell has exited, and Return restarts it.
    awaiting: bool,
    /// The terminal has keyboard focus, so its cursor is solid and blinks.
    focused: bool,
    /// When the cursor blink last restarted. It shows from here, so it never vanishes while the
    /// person types.
    blink_from: Instant,
    /// Text an input method is still composing, drawn at the cursor until it is committed.
    composing: String,
    /// The view's cell and the grid that fills it, for mapping the mouse and answering the
    /// program.
    cell: CellMetrics,
    grid: GridSize,
    /// Where the cursor was last drawn, in the view's pixels from its top left.
    cursor_rect: [f32; 4],
    /// The theme's colours, which change while the session runs.
    palette: Palette,
    /// The shell was told there is something new to draw and has not taken a frame since.
    told: bool,
    /// Input held until the shell waits at its prompt (`type_ahead`), and since when.
    #[cfg(unix)]
    typed_ahead: Option<(Vec<u8>, Instant)>,
}

impl State {
    /// Whether someone is watching the session: it is on screen with keyboard focus, which only
    /// happens in the window the person is at. A turn that ends while watched counts as seen.
    fn is_watched(&self) -> bool {
        self.shown && self.focused
    }

    fn text_area(&self) -> TextArea {
        TextArea {
            grid: self.grid,
            cell: (self.cell.width, self.cell.height),
        }
    }
}

/// Mouse interaction in progress.
#[derive(Default)]
struct Pointer {
    wheel: Wheel,
    /// The cell last reported to a mouse-tracking program, so a drag within one cell is reported
    /// once.
    reported: Option<(usize, usize)>,
}

/// What the session, its emulator and the threads behind it share.
struct Shared {
    state: Mutex<State>,
    /// The emulator, until the session stops. Never lock the emulator while holding `state`: the
    /// emulator calls its listener with its own lock held, and the listener takes `state`.
    engine: Mutex<Option<Arc<Engine>>>,
    pointer: Mutex<Pointer>,
    /// What the session runs, again when Return restarts it.
    launch: Launch,
    /// Where the session's turns are reported (`changes`).
    turns: crate::changes::turns::TurnTap,
}

impl Shared {
    fn new(
        launch: Launch,
        palette: Palette,
        view: ViewSize,
        turns: crate::changes::turns::TurnTap,
    ) -> Self {
        Self {
            state: Mutex::new(State {
                shown: true,
                session: crate::terminal::activity::Session::default(),
                awaiting: false,
                focused: false,
                blink_from: Instant::now(),
                composing: String::new(),
                cell: view.cell,
                grid: grid_size(view),
                cursor_rect: [0.0; 4],
                palette,
                told: false,
                #[cfg(unix)]
                typed_ahead: None,
            }),
            engine: Mutex::new(None),
            pointer: Mutex::new(Pointer::default()),
            launch,
            turns,
        }
    }

    /// A session that runs nothing, for the emulator's tests.
    #[cfg(test)]
    fn for_tests() -> Arc<Self> {
        let launch = Launch {
            file: String::new(),
            args: Vec::new(),
            env: Vec::new(),
            cwd: std::path::PathBuf::from("/"),
        };
        let view = ViewSize {
            width: 160,
            height: 60,
            cell: CellMetrics {
                width: 8.0,
                height: 15.0,
                baseline: 11.0,
                stroke: 1.0,
            },
        };
        Arc::new(Self::new(
            launch,
            Palette::on([17, 17, 17], [238, 238, 240], [60, 60, 90]),
            view,
            crate::changes::turns::TurnTap::default(),
        ))
    }

    fn engine(&self) -> Option<Arc<Engine>> {
        self.engine.relock().clone()
    }

    /// Tells the shell there is something new to draw, once until it takes the next frame.
    fn changed(&self) {
        let tell = {
            let mut state = self.state.relock();
            let tell = state.shown && !state.told;
            state.told |= tell;
            tell
        };
        if let Some(changed) = FRAME_CHANGED.get().filter(|_| tell) {
            changed();
        }
    }

    /// Sends bytes to the process, if one is running.
    fn send(&self, bytes: Vec<u8>) {
        if let Some(engine) = self.engine() {
            engine.send(bytes);
        }
    }

    /// Sends the input held by `Terminal::type_ahead` once the shell waits at its prompt
    /// (`at_prompt`), or once it has waited `TYPED_AHEAD_MOST` regardless.
    #[cfg(unix)]
    fn send_typed_ahead(&self, at_prompt: bool) {
        let due = {
            let mut state = self.state.relock();
            let ready = state
                .typed_ahead
                .as_ref()
                .is_some_and(|(_, since)| at_prompt || since.elapsed() >= TYPED_AHEAD_MOST);
            if ready {
                state.typed_ahead.take().map(|(bytes, _)| bytes)
            } else {
                None
            }
        };
        if let Some(bytes) = due {
            self.send(bytes);
        }
    }

    /// The emulator has new output. A session's first output ends its starting state.
    fn output_changed(&self) {
        self.heard(&crate::terminal::activity::Changes::default());
        self.changed();
    }

    /// The program set its title; empty when it reset it.
    fn titled(&self, title: String) {
        self.heard(&crate::terminal::activity::Changes {
            title: Some(title),
            ..Default::default()
        });
    }

    /// Applies what the program reported about itself and passes what matters to its turns on
    /// to the changes. A turn that ends while the session is on screen counts as seen.
    fn heard(&self, changes: &crate::terminal::activity::Changes) {
        use crate::changes::turns::Signal;
        let (changed, unhooked) = {
            let mut state = self.state.relock();
            let watched = state.is_watched();
            let changed = state.session.observe(changes, watched);
            let unhooked = (changed && !state.session.hooked()).then_some(state.session.activity);
            (changed, unhooked)
        };
        if let Some(activity) = changes.activity {
            self.turns.send(Signal::Hook(activity));
        }
        for edit in &changes.edits {
            self.turns
                .send(Signal::Edited(std::path::PathBuf::from(edit)));
        }
        if let Some(activity) = unhooked {
            self.turns.send(Signal::Unhooked(activity));
        }
        if changed {
            crate::notify_changed();
        }
    }

    /// Records what holds the terminal's foreground: `agent` when the agent changed (`Some(None)`
    /// for none now), how it was `launched`, and whether something the person started holds it
    /// (`busy`). Windows exposes no foreground program.
    #[cfg(unix)]
    fn foreground(
        &self,
        agent: Option<Option<crate::terminal::agent::Agent>>,
        busy: bool,
        launched: crate::terminal::activity::Launched,
    ) {
        let changed = {
            let mut state = self.state.relock();
            let watched = state.is_watched();
            let ran = agent.is_some_and(|agent| state.session.runs(agent, watched));
            if ran {
                state.session.launched(launched);
            }
            let held = state.session.holds(busy);
            ran || held
        };
        if changed {
            crate::notify_changed();
        }
    }

    /// The shell exited with `code` (`None` for a signal): says so under its last output and
    /// waits for Return.
    fn exited(&self, code: Option<i32>) {
        if let Some(engine) = self.engine() {
            engine.ended();
            let notice = crate::terminal::exit::ended_notice(crate::ffi::interface_language());
            engine.feed(notice.as_bytes());
        }
        {
            let mut state = self.state.relock();
            state.awaiting = true;
            let watched = state.is_watched();
            state.session.runs(None, watched);
            state
                .session
                .ended(crate::terminal::exit::Exit { code }, watched);
        }
        crate::notify_changed();
        self.changed();
    }

    /// The colour a program asked for, from the theme's palette.
    fn queried_colour(&self, queried: Queried) -> Rgb {
        self.state.relock().palette.queried(queried)
    }

    fn text_area(&self) -> TextArea {
        self.state.relock().text_area()
    }
}

pub struct Terminal {
    shared: Arc<Shared>,
}

impl Terminal {
    /// Starts a new session of `launch` on screen in `view`, showing `shown_first` before the
    /// shell's own output (`transcript::shown`; empty for nothing). Its turns are reported
    /// through `turns`.
    pub fn start(
        view: ViewSize,
        launch: Launch,
        palette: Palette,
        turns: crate::changes::turns::TurnTap,
        shown_first: &[u8],
    ) -> Self {
        let shared = Arc::new(Shared::new(launch, palette, view, turns));
        let engine = Arc::new(Engine::new(&shared, grid_size(view)));
        *shared.engine.relock() = Some(Arc::clone(&engine));
        if !shown_first.is_empty() {
            engine.feed(shown_first);
        }
        let area = shared.text_area();
        if let Err(error) = engine.spawn(&shared, &shared.launch, area) {
            eprintln!("terminal\tthe shell did not start: {error}");
        }
        Self { shared }
    }

    /// Shows the session in `view` again, after `detach`.
    pub fn attach(&self, view: ViewSize) {
        {
            let mut state = self.shared.state.relock();
            // Opening the terminal counts as seeing where the agent stopped.
            state.session.seen();
            state.shown = true;
            state.told = false;
        }
        self.resize(view);
        self.shared.changed();
    }

    /// Switches to `palette` from the next frame, when the app theme changes.
    pub fn set_palette(&self, palette: Palette) {
        self.shared.state.relock().palette = palette;
        self.shared.changed();
    }

    /// Takes the session off screen. The process and its screen keep running, so the session is
    /// as it was left when shown again.
    pub fn detach(&self) {
        // An off-screen terminal has no keyboard focus.
        self.focus(false);
        let mut state = self.shared.state.relock();
        state.composing.clear();
        state.shown = false;
    }

    /// Fits the grid to `view`. The emulator and the process hear of it only when the grid or
    /// its cell changed, so a drag that keeps the grid sends nothing.
    pub fn resize(&self, view: ViewSize) {
        let grid = grid_size(view);
        let area = {
            let mut state = self.shared.state.relock();
            if state.grid == grid && state.cell == view.cell {
                return;
            }
            state.grid = grid;
            state.cell = view.cell;
            state.text_area()
        };
        if let Some(engine) = self.shared.engine() {
            engine.resize(area);
        }
        self.shared.changed();
    }

    /// The frame to draw now, and when to draw again for the cursor's blink. Taking it lets
    /// `watch_frames` call again on the next change.
    pub fn frame(&self) -> Option<(Frame, Option<Duration>)> {
        let engine = self.shared.engine()?;
        let (palette, cell, focused, blink_from, composing) = {
            let mut state = self.shared.state.relock();
            state.told = false;
            (
                state.palette,
                state.cell,
                state.focused,
                state.blink_from,
                state.composing.clone(),
            )
        };
        let into_blink = blink_from.elapsed();
        let half = BLINK_HALF_PERIOD.as_millis();
        let view = crate::terminal::frame::View {
            palette,
            cell,
            focused,
            blink_on: (into_blink.as_millis() / half).is_multiple_of(2),
            composing: &composing,
        };
        let frame =
            engine.read(|cells, cursor| crate::terminal::frame::build(cells, cursor, &view));
        self.shared.state.relock().cursor_rect = frame.cursor_rect;
        let turn = frame.blinking.then(|| {
            let into = into_blink.as_millis() % half;
            Duration::from_millis((half - into) as u64 + 1)
        });
        Some((frame, turn))
    }

    /// The session's activity, for its card and row.
    pub fn activity(&self) -> crate::terminal::activity::Activity {
        self.shared.state.relock().session.activity
    }

    /// The agent the person started in the terminal, when one holds the foreground.
    pub fn agent(&self) -> Option<crate::terminal::agent::Agent> {
        self.shared.state.relock().session.agent
    }

    /// Whether something the person started holds the foreground, so the shell is not at its
    /// prompt. Always false on Windows, where ConPTY exposes no foreground program.
    pub fn busy(&self) -> bool {
        self.shared.state.relock().session.busy
    }

    /// The program's last title, for the sessions list; empty until it sets one.
    pub fn title(&self) -> String {
        self.shared.state.relock().session.title.clone()
    }

    fn modes(&self) -> Modes {
        self.shared
            .engine()
            .map_or_else(Modes::default, |engine| engine.modes())
    }

    /// Sends typed bytes to the process. Typing scrolls back to the newest output and clears the
    /// selection. After the shell has exited, Return starts it again and nothing else is sent.
    pub fn input(&self, bytes: &[u8]) {
        let restart = {
            let mut state = self.shared.state.relock();
            state.session.seen();
            state.blink_from = Instant::now();
            // Without hooks, Return is the best sign a turn starts.
            if !state.awaiting && bytes.contains(&b'\r') && !state.session.hooked() {
                self.shared
                    .turns
                    .send(crate::changes::turns::Signal::Entered);
            }
            if state.awaiting {
                let restart = bytes.contains(&b'\r');
                if restart {
                    state.awaiting = false;
                    state.session = crate::terminal::activity::Session::default();
                }
                Some(restart)
            } else {
                None
            }
        };
        match restart {
            Some(true) => self.restart(),
            Some(false) => {}
            None => {
                if let Some(engine) = self.shared.engine() {
                    engine.typed();
                    engine.send(bytes.to_vec());
                }
                self.shared.changed();
            }
        }
    }

    /// Starts the shell again after it exited.
    fn restart(&self) {
        let Some(engine) = self.shared.engine() else {
            return;
        };
        let area = self.shared.text_area();
        if let Err(error) = engine.spawn(&self.shared, &self.shared.launch, area) {
            eprintln!("terminal\tthe shell did not start again: {error}");
        }
        crate::notify_changed();
        self.shared.changed();
    }

    /// A non-text key: a named key, or a character with Control or Option held. Shift with Page Up,
    /// Page Down, Home or End scrolls the scrollback instead, unless a full-screen program owns the
    /// screen.
    pub fn key(&self, key: Key, modifiers: Modifiers) {
        let Some(engine) = self.shared.engine() else {
            return;
        };
        let modes = engine.modes();
        if modifiers.shift() && !modes.alt_screen {
            let scroll = match key {
                Key::PageUp => Some(Scroll::PageUp),
                Key::PageDown => Some(Scroll::PageDown),
                Key::Home => Some(Scroll::Top),
                Key::End => Some(Scroll::Bottom),
                _ => None,
            };
            if let Some(scroll) = scroll {
                engine.scroll(scroll);
                self.shared.changed();
                return;
            }
        }
        self.input(&encode_key(key, modifiers, modes));
    }

    /// A mouse event at `x`, `y` in view pixels from the top-left. `button`: 0 left, 1 middle,
    /// 2 right. `clicks` counts double and triple clicks. A mouse-tracking program gets the event
    /// unless Shift is held; otherwise the left button selects cells, words on a double click, or
    /// lines on a triple click.
    pub fn mouse(
        &self,
        action: MouseAction,
        button: u32,
        x: f32,
        y: f32,
        clicks: u32,
        modifiers: Modifiers,
    ) {
        let Some(engine) = self.shared.engine() else {
            return;
        };
        let (cell, grid) = {
            let state = self.shared.state.relock();
            (state.cell, state.grid)
        };
        let at = cell_at(x, y, (cell.width, cell.height), grid.columns);
        let line = visible_line(grid, at.line);
        let modes = engine.modes();
        if program_has_mouse(modes, modifiers) {
            self.report_mouse(action, button, (at.column, line), modifiers, modes);
            return;
        }
        if button != 0 {
            return;
        }
        engine.select(action, &at, line, clicks, modifiers);
        self.shared.changed();
    }

    /// Reports a mouse event at `cell` to a mouse-tracking program; a drag within the last reported
    /// cell is reported once.
    fn report_mouse(
        &self,
        action: MouseAction,
        button: u32,
        cell: (usize, usize),
        modifiers: Modifiers,
        modes: Modes,
    ) {
        let mut pointer = self.shared.pointer.relock();
        if action == MouseAction::Drag && pointer.reported == Some(cell) {
            return;
        }
        pointer.reported = Some(cell);
        if let Some(report) = encode_mouse(action, button, cell.0, cell.1, modifiers, modes) {
            self.shared.send(report);
        }
    }

    /// The wheel or trackpad moved by `delta`: view pixels when `precise` (a trackpad), else wheel
    /// notches; positive is towards older output. `x`, `y` is the pointer position. A
    /// mouse-tracking program gets wheel reports, a full-screen program without tracking gets arrow
    /// keys, and otherwise the scrollback moves.
    pub fn scroll(&self, delta: f32, precise: bool, x: f32, y: f32, modifiers: Modifiers) {
        let Some(engine) = self.shared.engine() else {
            return;
        };
        let (cell, grid) = {
            let state = self.shared.state.relock();
            (state.cell, state.grid)
        };
        let pixels = if precise {
            delta
        } else {
            delta * WHEEL_NOTCH_LINES * cell.height
        };
        let lines = self
            .shared
            .pointer
            .relock()
            .wheel
            .lines(pixels, cell.height);
        if lines == 0 {
            return;
        }
        let modes = engine.modes();
        if program_has_mouse(modes, modifiers) {
            let at = cell_at(x, y, (cell.width, cell.height), grid.columns);
            self.report_wheel(
                lines,
                (at.column, visible_line(grid, at.line)),
                modifiers,
                modes,
            );
        } else if modes.alt_screen && modes.alternate_scroll && !modifiers.shift() {
            self.shared.send(wheel_as_arrows(lines, modes));
        } else {
            engine.scroll(Scroll::Lines(lines));
            self.shared.changed();
        }
    }

    /// Reports `lines` of wheel movement at `cell` to a mouse-tracking program, one report per
    /// line.
    fn report_wheel(&self, lines: i32, cell: (usize, usize), modifiers: Modifiers, modes: Modes) {
        let action = if lines > 0 {
            MouseAction::WheelUp
        } else {
            MouseAction::WheelDown
        };
        if let Some(report) = encode_mouse(action, 0, cell.0, cell.1, modifiers, modes) {
            self.shared
                .send(report.repeat(lines.unsigned_abs() as usize));
        }
    }

    /// Types `bytes` into the shell once it waits at its prompt, not while it is still starting:
    /// input typed sooner is echoed twice, once as it arrives and again as the prompt reads it.
    /// Windows exposes no prompt to wait for, so it is typed at once there.
    pub fn type_ahead(&self, bytes: Vec<u8>) {
        #[cfg(unix)]
        {
            self.shared.state.relock().typed_ahead = Some((bytes, Instant::now()));
        }
        #[cfg(not(unix))]
        self.shared.send(bytes);
    }

    /// The command that opens this terminal's agent conversation again after a relaunch
    /// (`activity::Session::resume_command`).
    pub fn resume_command(&self) -> Option<crate::terminal::resume::Resume> {
        self.shared.state.relock().session.resume_command()
    }

    /// The whole session's text, scrollback included, as the app quits (`transcript::keep`).
    pub fn transcript(&self) -> String {
        self.shared
            .engine()
            .map(|engine| engine.everything_shown())
            .unwrap_or_default()
    }

    /// The selected text, or an empty string.
    pub fn copy(&self) -> String {
        self.shared
            .engine()
            .map(|engine| engine.selected_text())
            .unwrap_or_default()
    }

    /// Selects everything, the scrollback included.
    pub fn select_all(&self) {
        if let Some(engine) = self.shared.engine() {
            engine.select_all();
            self.shared.changed();
        }
    }

    /// Pastes text, wrapped in bracketed-paste markers when the program enabled them.
    pub fn paste(&self, text: &str) {
        let bracketed = self.modes().bracketed_paste;
        self.input(&paste_bytes(text, bracketed));
    }

    /// Sets whether the terminal has keyboard focus, notifying a program that asked. Regaining
    /// focus counts as seeing where the agent stopped.
    pub fn focus(&self, focused: bool) {
        let seen = {
            let mut state = self.shared.state.relock();
            if state.focused == focused {
                return;
            }
            state.focused = focused;
            state.blink_from = Instant::now();
            focused && state.session.seen()
        };
        self.shared.changed();
        if seen {
            crate::notify_changed();
        }
        if self.modes().focus_reports {
            self.shared.send(if focused {
                b"\x1b[I".to_vec()
            } else {
                b"\x1b[O".to_vec()
            });
        }
    }

    /// Sets the input method's composing text, shown at the cursor; empty when done.
    pub fn compose(&self, text: &str) {
        {
            let mut state = self.shared.state.relock();
            if state.composing == text {
                return;
            }
            text.clone_into(&mut state.composing);
            state.blink_from = Instant::now();
        }
        self.shared.changed();
    }

    /// The cursor's frame: x, y, width, height in view pixels from the top-left.
    pub fn cursor_rect(&self) -> [f32; 4] {
        self.shared.state.relock().cursor_rect
    }

    /// Ends the session's process. The emulator holds its listener, which holds this session,
    /// so letting go of the emulator breaks the cycle.
    pub fn stop(self) {
        let engine = self.shared.engine.relock().take();
        if let Some(engine) = engine {
            engine.shut_down();
        }
    }
}

/// The visible line nearest to `line`, which may be above or below the grid.
fn visible_line(grid: GridSize, line: i32) -> usize {
    line.clamp(0, grid.lines as i32 - 1) as usize
}

/// Whether a mouse-tracking program gets mouse events: yes, unless Shift is held.
fn program_has_mouse(modes: Modes, modifiers: Modifiers) -> bool {
    modes.mouse.is_some() && !modifiers.shift()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellMetrics = CellMetrics {
        width: 16.0,
        height: 30.0,
        baseline: 22.0,
        stroke: 2.0,
    };

    #[test]
    fn the_grid_fills_the_view_in_whole_cells() {
        let view = |width, height| ViewSize {
            width,
            height,
            cell: CELL,
        };
        assert_eq!(
            grid_size(view(1000, 620)),
            GridSize {
                columns: 62,
                lines: 20
            }
        );
        assert_eq!(
            grid_size(view(1, 1)),
            GridSize {
                columns: MIN_COLUMNS,
                lines: MIN_LINES
            }
        );
    }

    /// Runs a shell session at 800 by 240 px, types `command` once the prompt is drawn, and
    /// returns the text once `answer` appears on its own line, plus the session.
    fn session_answering(command: &[u8], answer: &str) -> (Terminal, String) {
        use crate::terminal::launch::{build_launch, choose_shell};
        let folder = std::env::temp_dir();
        let shell = choose_shell(None);
        let environment: Vec<(String, String)> = std::env::vars().collect();
        let launch = build_launch(&folder, &environment, &shell, &[]);
        let palette = Palette::on([17, 17, 17], [238, 238, 240], [60, 60, 90]);
        let view = ViewSize {
            width: 800,
            height: 240,
            cell: CELL,
        };
        let terminal = Terminal::start(
            view,
            launch,
            palette,
            crate::changes::turns::TurnTap::default(),
            &[],
        );
        let started = Instant::now();
        let mut typed = false;
        loop {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "the shell never answered"
            );
            std::thread::sleep(Duration::from_millis(200));
            let text = terminal
                .shared
                .engine()
                .map(|engine| engine.everything_shown())
                .unwrap_or_default();
            // Typed once the shell has drawn its prompt.
            if !typed && !text.trim().is_empty() {
                terminal.input(command);
                typed = true;
            }
            if text.lines().any(|line| line.trim() == answer) {
                return (terminal, text);
            }
        }
    }

    /// A session runs under the platform's terminal (a pty, or ConPTY on Windows), reads input,
    /// and its frame sets the answer as text on the theme's background.
    #[test]
    fn a_session_runs_and_its_frame_shows_what_it_printed() {
        // Use shell arithmetic, so the answer comes from the shell and not the echoed input.
        let command: &[u8] = if cfg!(windows) {
            b"echo \"northroom-$(6*7)\"\r"
        } else {
            b"echo northroom-$((6*7))\r"
        };
        let (terminal, _) = session_answering(command, "northroom-42");
        let (frame, _) = terminal.frame().expect("a running session has a frame");
        assert_eq!(frame.background, cells::rgb(17, 17, 17));
        assert!(frame.text.contains("northroom-42"), "{}", frame.text);
        // The grid is 800 / 16 by 240 / 30.
        assert_eq!(
            terminal.shared.text_area().grid,
            GridSize {
                columns: 50,
                lines: 8
            }
        );
        terminal.stop();
    }

    /// The grid changes only when the view gains or loses a whole cell.
    #[test]
    fn a_resize_within_a_cell_changes_nothing() {
        let shared = Shared::for_tests();
        let terminal = Terminal {
            shared: Arc::clone(&shared),
        };
        let before = shared.text_area();
        let cell = shared.state.relock().cell;
        let view = |width, height| ViewSize {
            width,
            height,
            cell,
        };
        terminal.resize(view(167, 74));
        assert_eq!(shared.text_area(), before);
        terminal.resize(view(168, 75));
        assert_eq!(
            shared.text_area().grid,
            GridSize {
                columns: 21,
                lines: 5
            }
        );
    }

    /// A Windows session under ConPTY gets the variables the terminal keeps and Northroom's own
    /// (`terminal::launch`). The emulator library then adds the app's other variables, as Zed,
    /// VS Code and Alacritty pass theirs on (DECISIONS.md), so that is not checked here.
    #[cfg(windows)]
    #[test]
    fn a_windows_session_holds_what_is_kept_and_northrooms_own() {
        // The test's own variable, which no other test reads.
        std::env::set_var("LC_NORTHROOM_PROBE", "kept");
        let (terminal, _) = session_answering(
            b"echo \"[$env:LC_NORTHROOM_PROBE][$env:NORTHROOM_TERMINAL]\"\r",
            "[kept][1]",
        );
        terminal.stop();
    }
}
