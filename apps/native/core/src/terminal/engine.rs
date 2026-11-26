//! The emulator and the process behind a terminal: `alacritty_terminal` runs the shell on a
//! pseudo-terminal, parses its output on its own thread and keeps the grid and scrollback.
//!
//! This is the only module that knows that library. It takes and gives Northroom's own types
//! (`cells`), so another emulator replaces this module alone: the session (`terminal`), the
//! frame (`frame`) and the boundary never see the library's types.

use super::cells::{
    Cell, Colour, Cursor, CursorShape, GridSize, Modes, MouseTracking, Rgb as Rgb8, Scroll,
    SelectionKind, Side as CellSide, Style, Underline,
};
use super::input::{CellAt, Modifiers, MouseAction};
use super::launch::Launch;
use super::{Queried, Shared, TextArea};
use crate::Relock;
use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Indexed, Scroll as GridScroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionRange, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::{Cell as GridCell, Flags};
use alacritty_terminal::term::{viewport_to_point, Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{
    Color, CursorShape as GridCursorShape, CursorStyle, NamedColor, Processor, Rgb,
};
use northroom_media::{MediaError, MediaResult};
use std::borrow::Cow;
use std::sync::{Arc, Mutex};
#[cfg(unix)]
use std::time::{Duration, Instant};

const SCROLLBACK_LINES: usize = 10_000;

/// The grid size as the emulator measures it.
#[derive(Clone, Copy)]
struct Dims(GridSize);

impl Dimensions for Dims {
    fn total_lines(&self) -> usize {
        self.0.lines
    }

    fn screen_lines(&self) -> usize {
        self.0.lines
    }

    fn columns(&self) -> usize {
        self.0.columns
    }
}

fn window_size(area: TextArea) -> WindowSize {
    WindowSize {
        num_lines: area.grid.lines as u16,
        num_cols: area.grid.columns as u16,
        cell_width: area.cell.0 as u16,
        cell_height: area.cell.1 as u16,
    }
}

fn terminal_error(action: &str, cause: impl std::fmt::Display) -> MediaError {
    MediaError(format!("{action}: {cause}"))
}

/// A session's emulator and the process feeding it.
pub(super) struct Engine {
    term: Arc<FairMutex<Term<Listener>>>,
    /// Set while a process runs; input and replies go through it.
    sender: Mutex<Option<EventLoopSender>>,
}

impl Engine {
    /// An emulator `grid` large that reports to `shared`, with no process yet.
    pub(super) fn new(shared: &Arc<Shared>, grid: GridSize) -> Self {
        let term = Term::new(
            Config {
                scrolling_history: SCROLLBACK_LINES,
                // A blinking block until the program asks for another style.
                default_cursor_style: CursorStyle {
                    shape: GridCursorShape::Block,
                    blinking: true,
                },
                ..Config::default()
            },
            &Dims(grid),
            Listener(Arc::clone(shared)),
        );
        Self {
            term: Arc::new(FairMutex::new(term)),
            sender: Mutex::new(None),
        }
    }

    /// Starts `launch` on a pseudo-terminal `area` large, read by this emulator.
    pub(super) fn spawn(
        &self,
        shared: &Arc<Shared>,
        launch: &Launch,
        area: TextArea,
    ) -> MediaResult<()> {
        let pty = tty::new(&process_options(launch), window_size(area), 0)
            .map_err(|cause| terminal_error("start the shell process", cause))?;
        #[cfg(unix)]
        watch_foreground(&pty, shared);
        // Tap the output on its way to the emulator, for what the emulator drops.
        let pty = TappedPty::new(pty, Arc::clone(shared));
        let event_loop = EventLoop::new(
            Arc::clone(&self.term),
            Listener(Arc::clone(shared)),
            pty,
            false,
            false,
        )
        .map_err(|cause| terminal_error("start the terminal reader", cause))?;
        *self.sender.relock() = Some(event_loop.channel());
        let _reader = event_loop.spawn();
        Ok(())
    }

    /// The process has ended; nothing more is sent to it.
    pub(super) fn ended(&self) {
        *self.sender.relock() = None;
    }

    /// Ends the process and its reader.
    pub(super) fn shut_down(&self) {
        if let Some(sender) = self.sender.relock().take() {
            let _ = sender.send(Msg::Shutdown);
        }
    }

    /// Sends bytes to the process, if one is running.
    pub(super) fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        if let Some(sender) = self.sender.relock().as_ref() {
            let _ = sender.send(Msg::Input(Cow::Owned(bytes)));
        }
    }

    /// Resizes the grid to `area` and tells the process.
    pub(super) fn resize(&self, area: TextArea) {
        self.term.lock().resize(Dims(area.grid));
        if let Some(sender) = self.sender.relock().as_ref() {
            let _ = sender.send(Msg::Resize(window_size(area)));
        }
    }

    /// Shows `bytes` as if the process had written them: the notice that the shell ended.
    pub(super) fn feed(&self, bytes: &[u8]) {
        let mut parser: Processor = Processor::new();
        parser.advance(&mut *self.term.lock(), bytes);
    }

    pub(super) fn modes(&self) -> Modes {
        modes_of(*self.term.lock().mode())
    }

    pub(super) fn scroll(&self, scroll: Scroll) {
        self.term.lock().scroll_display(match scroll {
            Scroll::Lines(lines) => GridScroll::Delta(lines),
            Scroll::PageUp => GridScroll::PageUp,
            Scroll::PageDown => GridScroll::PageDown,
            Scroll::Top => GridScroll::Top,
            Scroll::Bottom => GridScroll::Bottom,
        });
    }

    /// Typing clears the selection and shows the newest output.
    pub(super) fn typed(&self) {
        let mut term = self.term.lock();
        term.selection = None;
        term.scroll_display(GridScroll::Bottom);
    }

    /// Left-button selection at `at` on visible `line`: cells, words on a double click, lines on
    /// a triple click; Shift extends the selection. Dragging past the top or bottom scrolls one
    /// line per event: older output above the top, newer below the bottom.
    pub(super) fn select(
        &self,
        action: MouseAction,
        at: &CellAt,
        line: usize,
        clicks: u32,
        modifiers: Modifiers,
    ) {
        let mut term = self.term.lock();
        if action == MouseAction::Drag && at.line != line as i32 {
            term.scroll_display(GridScroll::Delta(-at.line.signum()));
        }
        let point = viewport_to_point(
            term.grid().display_offset(),
            Point::new(line, Column(at.column)),
        );
        let side = side_of(at.side);
        match action {
            MouseAction::Press => press_selection(&mut term, point, side, clicks, modifiers),
            MouseAction::Drag => {
                if let Some(selection) = term.selection.as_mut() {
                    selection.update(point, side);
                }
            }
            MouseAction::Release | MouseAction::WheelUp | MouseAction::WheelDown => {
                // A click that selected nothing leaves nothing selected.
                if term.selection.as_ref().is_some_and(Selection::is_empty) {
                    term.selection = None;
                }
            }
        }
    }

    /// The selected text, or an empty string.
    pub(super) fn selected_text(&self) -> String {
        self.term.lock().selection_to_string().unwrap_or_default()
    }

    /// Selects everything, the scrollback included.
    pub(super) fn select_all(&self) {
        let mut term = self.term.lock();
        let (top, bottom) = (term.grid().topmost_line(), term.grid().bottommost_line());
        let last = term.grid().last_column();
        let mut selection = Selection::new(
            SelectionType::Simple,
            Point::new(top, Column(0)),
            Side::Left,
        );
        selection.update(Point::new(bottom, last), Side::Right);
        term.selection = Some(selection);
    }

    /// The whole session's text, scrollback included, read without selecting.
    pub(super) fn everything_shown(&self) -> String {
        let term = self.term.lock();
        let (top, bottom) = (term.grid().topmost_line(), term.grid().bottommost_line());
        let last = term.grid().last_column();
        term.bounds_to_string(Point::new(top, Column(0)), Point::new(bottom, last))
    }

    /// Hands `build` the visible cells, top to bottom and left to right, and the cursor, with
    /// the emulator held still. `build` must not lock the session's state.
    pub(super) fn read<T>(
        &self,
        build: impl FnOnce(&mut dyn Iterator<Item = Cell<'_>>, Cursor) -> T,
    ) -> T {
        let term = self.term.lock();
        let content = term.renderable_content();
        let offset = content.display_offset as i32;
        let point = content.cursor.point;
        let cursor = Cursor {
            line: point.line.0 + offset,
            column: point.column.0,
            shape: shape_of(content.cursor.shape),
            blinking: term.cursor_style().blinking,
            wide: term.grid()[point].flags.contains(Flags::WIDE_CHAR),
        };
        let selection = content.selection;
        let mut cells = content
            .display_iter
            .filter(|indexed| !is_spacer(indexed.flags))
            .map(|indexed| cell_of(&indexed, offset, selection.as_ref()));
        build(&mut cells, cursor)
    }
}

/// The second half of a wide character, or the blank a wide character left at a line's end.
fn is_spacer(flags: Flags) -> bool {
    flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
}

fn cell_of<'a>(
    indexed: &Indexed<&'a GridCell>,
    offset: i32,
    selection: Option<&SelectionRange>,
) -> Cell<'a> {
    let flags = indexed.flags;
    let wide = flags.contains(Flags::WIDE_CHAR);
    Cell {
        line: (indexed.point.line.0 + offset).max(0) as usize,
        column: indexed.point.column.0,
        character: indexed.c,
        marks: indexed.cell.zerowidth().unwrap_or(&[]),
        wide,
        foreground: colour_of(indexed.fg),
        background: colour_of(indexed.bg),
        underline_colour: indexed.cell.underline_color().map(colour_of),
        style: style_of(flags),
        selected: is_selected(selection, indexed.point, wide),
    }
}

/// Whether a cell is inside the selection. A wide character is one cell here and two in the
/// grid; selecting either half selects it.
fn is_selected(selection: Option<&SelectionRange>, point: Point, wide: bool) -> bool {
    selection.is_some_and(|range| {
        range.contains(point) || (wide && range.contains(Point::new(point.line, point.column + 1)))
    })
}

fn style_of(flags: Flags) -> Style {
    let underline = if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.intersects(Flags::ALL_UNDERLINES) {
        Underline::Single
    } else {
        Underline::None
    };
    Style {
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        dim: flags.contains(Flags::DIM),
        inverse: flags.contains(Flags::INVERSE),
        hidden: flags.contains(Flags::HIDDEN),
        strikeout: flags.contains(Flags::STRIKEOUT),
        underline,
    }
}

fn colour_of(color: Color) -> Colour {
    match color {
        Color::Spec(rgb) => Colour::Rgb(Rgb8 {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
        Color::Indexed(index) => Colour::Indexed(index),
        Color::Named(name) => named_colour(name),
    }
}

fn named_colour(name: NamedColor) -> Colour {
    let index = name as usize;
    let dim = NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize;
    match name {
        NamedColor::Background => Colour::Background,
        NamedColor::Foreground
        | NamedColor::BrightForeground
        | NamedColor::DimForeground
        | NamedColor::Cursor => Colour::Foreground,
        // The dim colours reuse the normal ones.
        _ if dim.contains(&index) => Colour::Indexed((index - dim.start()) as u8),
        _ => Colour::Indexed(index as u8),
    }
}

fn shape_of(shape: GridCursorShape) -> CursorShape {
    match shape {
        GridCursorShape::Block => CursorShape::Block,
        GridCursorShape::HollowBlock => CursorShape::HollowBlock,
        GridCursorShape::Beam => CursorShape::Beam,
        GridCursorShape::Underline => CursorShape::Underline,
        GridCursorShape::Hidden => CursorShape::Hidden,
    }
}

fn modes_of(mode: TermMode) -> Modes {
    let mouse = if mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION) {
        Some(MouseTracking::Drags)
    } else if mode.intersects(TermMode::MOUSE_MODE) {
        Some(MouseTracking::Clicks)
    } else {
        None
    };
    Modes {
        app_cursor: mode.contains(TermMode::APP_CURSOR),
        mouse,
        sgr_mouse: mode.contains(TermMode::SGR_MOUSE),
        alt_screen: mode.contains(TermMode::ALT_SCREEN),
        alternate_scroll: mode.contains(TermMode::ALTERNATE_SCROLL),
        bracketed_paste: mode.contains(TermMode::BRACKETED_PASTE),
        focus_reports: mode.contains(TermMode::FOCUS_IN_OUT),
    }
}

fn side_of(side: CellSide) -> Side {
    match side {
        CellSide::Left => Side::Left,
        CellSide::Right => Side::Right,
    }
}

/// A press starts a selection, or with Shift moves the end of the existing one.
fn press_selection(
    term: &mut Term<Listener>,
    point: Point,
    side: Side,
    clicks: u32,
    modifiers: Modifiers,
) {
    let extends = modifiers.shift() && term.selection.is_some();
    match term.selection.as_mut().filter(|_| extends) {
        Some(selection) => selection.update(point, side),
        None => {
            let kind = match SelectionKind::of_clicks(clicks) {
                SelectionKind::Cells => SelectionType::Simple,
                SelectionKind::Word => SelectionType::Semantic,
                SelectionKind::Lines => SelectionType::Lines,
            };
            term.selection = Some(Selection::new(kind, point, side));
        }
    }
}

/// What a program asks for by colour index: the 256-colour table, then text, background and
/// cursor.
fn queried(index: usize) -> Queried {
    match u8::try_from(index) {
        Ok(index) => Queried::Indexed(index),
        Err(_) if index == NamedColor::Background as usize => Queried::Background,
        Err(_) => Queried::Foreground,
    }
}

/// The emulator's event listener: the session hears what changed, and replies go back to the
/// process. It is called with the emulator locked, so it never locks the emulator.
#[derive(Clone)]
struct Listener(Arc<Shared>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::Wakeup => self.0.output_changed(),
            Event::Title(title) => self.0.titled(title),
            Event::ResetTitle => self.0.titled(String::new()),
            Event::ChildExit(status) => self.0.exited(status.code()),
            Event::PtyWrite(text) => self.0.send(text.into_bytes()),
            // A program querying the background or text colour (Codex does, to adapt) gets the
            // app's colours.
            Event::ColorRequest(index, format) => {
                let colour = self.0.queried_colour(queried(index));
                self.0.send(
                    format(Rgb {
                        r: colour.r,
                        g: colour.g,
                        b: colour.b,
                    })
                    .into_bytes(),
                );
            }
            Event::TextAreaSizeRequest(format) => {
                self.0
                    .send(format(window_size(self.0.text_area())).into_bytes());
            }
            Event::CursorBlinkingChange => self.0.changed(),
            _ => {}
        }
    }
}

/// The emulator's process with a tap on its output. The emulator reports a program's title and
/// exit but drops its notifications and the escape sequence Northroom's launch hooks write to
/// report activity. Output is read through the process's own reader (a Unix pty, or ConPTY's
/// pipe on Windows) and inspected here on the way. Everything else passes straight through.
struct TappedPty {
    inner: tty::Pty,
    signals: crate::terminal::activity::Signals,
    /// Which agent the person started in the shell, checked as output is read.
    #[cfg(unix)]
    agent: crate::terminal::agent::Watch,
    shared: Arc<Shared>,
}

impl TappedPty {
    fn new(inner: tty::Pty, shared: Arc<Shared>) -> Self {
        Self {
            #[cfg(unix)]
            agent: crate::terminal::agent::Watch::new(
                i32::try_from(inner.child().id()).unwrap_or_default(),
            ),
            inner,
            signals: crate::terminal::activity::Signals::default(),
            shared,
        }
    }

    /// Checks which program holds the terminal now and tells the session. ConPTY exposes no
    /// foreground program, so this does nothing on Windows.
    #[cfg(unix)]
    fn follow_agent(&mut self) {
        use crate::terminal::agent::{arguments_of, folder_of, foreground_group};
        use std::os::fd::AsRawFd;
        let Some(group) = foreground_group(self.inner.file().as_raw_fd()) else {
            return;
        };
        let busy = !self.agent.is_shell(group);
        let agent = self.agent.look(group, Instant::now(), arguments_of);
        // The folder a newly found agent works in, which its latest conversation is kept for.
        let launched = crate::terminal::activity::Launched {
            arguments: self.agent.arguments().to_vec(),
            folder: agent.flatten().and_then(|_| folder_of(group)),
        };
        self.shared.foreground(agent, busy, launched);
    }
}

impl std::io::Read for TappedPty {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        use tty::EventedReadWrite;
        let count = self.inner.reader().read(buffer)?;
        // Check before applying what was read, so an agent's first output is attributed to it,
        // not to the previous program.
        #[cfg(unix)]
        self.follow_agent();
        let mut changes = self
            .signals
            .observe(&String::from_utf8_lossy(&buffer[..count]));
        // The emulator supplies complete titles; a chunk here may end mid-title.
        changes.title = None;
        if changes.notification.is_some()
            || changes.activity.is_some()
            || changes.conversation.is_some()
            || changes.conversation_folder.is_some()
        {
            self.shared.heard(&changes);
        }
        Ok(count)
    }
}

impl tty::EventedReadWrite for TappedPty {
    // The emulator reads through this, which reads from the process's own reader.
    type Reader = Self;
    type Writer = <tty::Pty as tty::EventedReadWrite>::Writer;

    unsafe fn register(
        &mut self,
        poller: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> std::io::Result<()> {
        // SAFETY: the process outlives its registration, as the caller promises of `self`.
        unsafe { self.inner.register(poller, interest, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> std::io::Result<()> {
        self.inner.reregister(poller, interest, mode)
    }

    fn deregister(&mut self, poller: &Arc<polling::Poller>) -> std::io::Result<()> {
        self.inner.deregister(poller)
    }

    fn reader(&mut self) -> &mut Self {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self.inner.writer()
    }
}

impl tty::EventedPty for TappedPty {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        self.inner.next_child_event()
    }
}

impl alacritty_terminal::event::OnResize for TappedPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.inner.on_resize(window_size);
    }
}

/// The emulator options for the process `launch` describes. On macOS and Linux it gets exactly
/// the launch's environment and nothing from this process (`env -i` starts empty, sets each
/// variable, then runs the shell).
#[cfg(unix)]
fn process_options(launch: &Launch) -> tty::Options {
    let mut arguments = vec!["-i".to_owned()];
    arguments.extend(
        launch
            .env
            .iter()
            .map(|(name, value)| format!("{name}={value}")),
    );
    arguments.push(launch.file.clone());
    arguments.extend(launch.args.iter().skip(1).cloned());
    tty::Options {
        shell: Some(tty::Shell::new("/usr/bin/env".to_owned(), arguments)),
        working_directory: Some(launch.cwd.clone()),
        drain_on_exit: false,
        env: std::collections::HashMap::new(),
    }
}

/// On Windows the process runs under ConPTY with this process's environment plus the launch's
/// variables, as a new Windows Terminal tab inherits Explorer's environment
/// (`terminal::launch`).
#[cfg(windows)]
fn process_options(launch: &Launch) -> tty::Options {
    tty::Options {
        shell: Some(tty::Shell::new(
            launch.file.clone(),
            launch.args.iter().skip(1).cloned().collect(),
        )),
        working_directory: Some(launch.cwd.clone()),
        drain_on_exit: false,
        env: launch.env.iter().cloned().collect(),
        escape_args: true,
    }
}

/// How often to check whether something the person started holds the terminal's foreground.
#[cfg(unix)]
const FOREGROUND_EVERY: Duration = Duration::from_secs(1);

/// Checks every `FOREGROUND_EVERY` whether something the person started holds the foreground,
/// so a silent program is still noticed; output is also checked as it is read
/// (`TappedPty::follow_agent`). Stops once the session or its shell is gone.
#[cfg(unix)]
fn watch_foreground(pty: &tty::Pty, shared: &Arc<Shared>) {
    use std::os::fd::{AsFd, AsRawFd};
    let Ok(terminal) = pty.file().as_fd().try_clone_to_owned() else {
        return;
    };
    let shell = i32::try_from(pty.child().id()).unwrap_or_default();
    let shared = Arc::downgrade(shared);
    let watching = move || loop {
        std::thread::sleep(FOREGROUND_EVERY);
        let Some(shared) = shared.upgrade() else {
            return;
        };
        if !look_at_foreground(&shared, terminal.as_raw_fd(), shell) {
            return;
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("northroom-terminal-foreground".into())
        .spawn(watching)
    {
        eprintln!("terminal\tthe foreground is looked at only as output comes: {error}");
    }
}

/// Records whether something the person started holds the foreground of the terminal whose
/// app side is `terminal`, with `shell` as the shell's process. Returns whether the shell is
/// still alive.
#[cfg(unix)]
fn look_at_foreground(shared: &Shared, terminal: std::os::fd::RawFd, shell: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists; nothing is sent.
    let alive = unsafe { libc::kill(shell, 0) } == 0;
    let foreground = crate::terminal::agent::foreground_group(terminal);
    let busy = alive && foreground.is_some_and(|group| group != shell);
    shared.foreground(None, busy, crate::terminal::activity::Launched::default());
    // A shell reading a line at its prompt turns the terminal's echo off to draw it itself.
    let at_prompt = alive && foreground == Some(shell) && !echoes(terminal);
    shared.send_typed_ahead(at_prompt);
    alive
}

/// Whether the terminal whose app side is `terminal` echoes what is typed, as it does until a
/// shell takes the line itself. True when it can't be read, so nothing waits on it.
#[cfg(unix)]
fn echoes(terminal: std::os::fd::RawFd) -> bool {
    // SAFETY: an all-zero termios is a valid value for the call to overwrite.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: reads the settings of a descriptor the caller holds open into `settings`.
    let read = unsafe { libc::tcgetattr(terminal, &mut settings) } == 0;
    !read || settings.c_lflag & libc::ECHO != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::Line;

    /// An emulator 20 by 4 that has shown `output`, with no process.
    fn showing(output: &str) -> Engine {
        let engine = Engine::new(
            &Shared::for_tests(),
            GridSize {
                columns: 20,
                lines: 4,
            },
        );
        engine.feed(output.as_bytes());
        engine
    }

    fn select(
        engine: &Engine,
        kind: SelectionType,
        from: (i32, usize),
        to: (i32, usize),
    ) -> String {
        let mut selection =
            Selection::new(kind, Point::new(Line(from.0), Column(from.1)), Side::Left);
        selection.update(Point::new(Line(to.0), Column(to.1)), Side::Right);
        engine.term.lock().selection = Some(selection);
        engine.selected_text()
    }

    #[test]
    fn a_wide_character_is_one_cell_covering_two() {
        let engine = showing("a日b");
        let line: Vec<(usize, char, bool)> = engine.read(|cells, _| {
            cells
                .take_while(|cell| cell.line == 0)
                .take(3)
                .map(|cell| (cell.column, cell.character, cell.wide))
                .collect()
        });
        assert_eq!(line, [(0, 'a', false), (1, '日', true), (3, 'b', false)]);
    }

    #[test]
    fn a_selection_becomes_its_text() {
        let engine = showing("hello world\r\nsecond 日本 line");
        // Cells, as dragged.
        assert_eq!(
            select(&engine, SelectionType::Simple, (0, 0), (0, 4)),
            "hello"
        );
        // Across the line break.
        assert_eq!(
            select(&engine, SelectionType::Simple, (0, 6), (1, 5)),
            "world\nsecond"
        );
        // A double click takes the word, a triple the line.
        assert_eq!(
            select(&engine, SelectionType::Semantic, (0, 8), (0, 8)),
            "world"
        );
        assert_eq!(
            select(&engine, SelectionType::Lines, (0, 3), (0, 3)),
            "hello world\n"
        );
        // Wide characters come out once each, whichever half the drag ended on.
        assert_eq!(
            select(&engine, SelectionType::Simple, (1, 7), (1, 10)),
            "日本"
        );
        assert_eq!(
            select(&engine, SelectionType::Simple, (1, 7), (1, 9)),
            "日本"
        );
    }

    #[test]
    fn either_half_selects_a_wide_cell() {
        let range = SelectionRange::new(
            Point::new(Line(0), Column(2)),
            Point::new(Line(0), Column(4)),
            false,
        );
        let at = |column| Point::new(Line(0), Column(column));
        assert!(is_selected(Some(&range), at(2), false));
        // A wide character at column 1 covers column 2.
        assert!(is_selected(Some(&range), at(1), true));
        assert!(!is_selected(Some(&range), at(1), false));
        assert!(!is_selected(None, at(2), false));
    }

    #[test]
    fn cells_carry_the_programs_colours_and_style() {
        let engine = showing("\x1b[1;31mA\x1b[0;2;4mB\x1b[0;7;38;2;1;2;3mC\x1b[0;45mD");
        let cells: Vec<(Colour, Colour, Style)> = engine.read(|cells, _| {
            cells
                .take(4)
                .map(|cell| (cell.foreground, cell.background, cell.style))
                .collect()
        });
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let dim_underlined = Style {
            dim: true,
            underline: Underline::Single,
            ..Style::default()
        };
        let inverse = Style {
            inverse: true,
            ..Style::default()
        };
        assert_eq!(
            cells,
            [
                (Colour::Indexed(1), Colour::Background, bold),
                (Colour::Foreground, Colour::Background, dim_underlined),
                (
                    Colour::Rgb(Rgb8 { r: 1, g: 2, b: 3 }),
                    Colour::Background,
                    inverse
                ),
                (Colour::Foreground, Colour::Indexed(5), Style::default()),
            ]
        );
        assert_eq!(named_colour(NamedColor::DimRed), Colour::Indexed(1));
        assert_eq!(named_colour(NamedColor::BrightCyan), Colour::Indexed(14));
    }

    #[test]
    fn the_cursor_follows_the_output_and_the_programs_style() {
        let engine = showing("ab\r\nc\x1b[6 q");
        let cursor = engine.read(|_, cursor| cursor);
        assert_eq!(
            cursor,
            Cursor {
                line: 1,
                column: 1,
                shape: CursorShape::Beam,
                blinking: false,
                wide: false,
            }
        );
        let engine = showing("\x1b[?25l");
        assert_eq!(engine.read(|_, cursor| cursor.shape), CursorShape::Hidden);
    }

    #[test]
    fn a_programs_modes_are_read_in_northrooms_terms() {
        let engine = showing("\x1b[?1h\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1004h");
        assert_eq!(
            engine.modes(),
            Modes {
                app_cursor: true,
                mouse: Some(MouseTracking::Drags),
                sgr_mouse: true,
                alt_screen: false,
                // On by default.
                alternate_scroll: true,
                bracketed_paste: true,
                focus_reports: true,
            }
        );
        let engine = showing("\x1b[?1000h\x1b[?1049h");
        let modes = engine.modes();
        assert_eq!(
            (modes.mouse, modes.alt_screen),
            (Some(MouseTracking::Clicks), true)
        );
    }
}
