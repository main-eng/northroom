//! A terminal, filling the tab, beside its column, or as one of several tiles (`terminals`). It
//! draws the core's frame of the grid with the rest of the window, as Zed's terminal element
//! does: cell backgrounds and lines as quads, and each run of text set in the terminal font by
//! GPUI's text system with every glyph on its cell. It forwards input to the core: keys,
//! composed text, mouse, wheel and clipboard. The core owns the emulation, the process, what is
//! drawn where and what each key sends (`nr_terminal_*`).
//!
//! The shell measures the terminal font's cell and sends the core the view's size at most once
//! a frame, while drawing; the core changes the grid only when a whole cell is gained or lost.

use super::{Ask, Screen};
use crate::script::probe::Probe;
use crate::script::{Answer, Pointer, Route, Step, Table};
use gpui::{
    App, Bounds, Context, CursorStyle, DispatchPhase, ElementInputHandler, EntityInputHandler,
    EventEmitter, FocusHandle, Font, FontFallbacks, FontFeatures, FontStyle, FontWeight,
    HitboxBehavior, InteractiveElement, IntoElement, KeyDownEvent, Keystroke, Modifiers,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollEvent, SharedString, Styled, Task, TextAlign, TextRun,
    UTF16Selection, Window, canvas, point, px, size,
};
use kit::lists::chip;
use kit::nr::*;
use kit::platform::{read_clipboard, write_clipboard};
use std::ops::Range;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub struct Terminal {
    focus: FocusHandle,
    frame: Bounds<Pixels>,
    /// The terminal file shown: `None` for the tab's open file, or a tiled terminal's id.
    file: Option<String>,
    /// `NR_NO_TERMINAL` disables the terminal, for comparing against a run without one.
    enabled: bool,
    /// The terminal font's cell, and the pixels per point it was measured at.
    cell: Option<(f32, NrTerminalCell)>,
    /// The view as last sent to the core; `None` before the core shows the session.
    attached: Option<NrTerminalView>,
    /// The core's number for this terminal, used to detach it (`nr_terminal_detach`).
    shown: u64,
    /// The focus state last sent to the core.
    focused: Option<bool>,
    /// Text an input method is composing, such as a dead key awaiting its letter or kana before
    /// kanji conversion. The core draws it at the cursor.
    marked: String,
    /// The button held over the terminal; moves while it is held are drags.
    held: Option<MouseButton>,
    /// The redraw due when the cursor's blink turns.
    blink: Option<Task<()>>,
}

impl Terminal {
    /// The terminal for the tab's open file.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::showing(None, window, cx)
    }

    /// The terminal for file `id`, as one of several tiles.
    pub fn tile(id: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::showing(Some(id.to_owned()), window, cx)
    }

    fn showing(file: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        cx.on_blur(&focus, window, |terminal, window, cx| {
            terminal.unmark(window, cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |_, _, cx| cx.notify())
            .detach();
        // Redraw when a terminal has something new to show (the core keeps only the first
        // watcher).
        nr_terminal_watch_frames(frame_changed);
        Self {
            focus,
            frame: Bounds::default(),
            file,
            enabled: nr_script_run().terminal,
            cell: None,
            attached: None,
            shown: 0,
            focused: None,
            marked: String::new(),
            held: None,
            blink: None,
        }
    }

    /// Sets the tile's window frame.
    pub fn set_frame(&mut self, frame: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    /// The terminal's focus handle.
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// The terminal font's cell at the window's scale, measured once per scale.
    fn cell(&mut self, window: &Window) -> NrTerminalCell {
        let scale = window.scale_factor();
        if let Some((measured, cell)) = self.cell
            && measured == scale
        {
            return cell;
        }
        let cell = measure_cell(window, scale);
        self.cell = Some((scale, cell));
        cell
    }

    /// Shows the terminal's session in `view` and keeps the core's number for it.
    fn attach(&mut self, view: NrTerminalView) {
        self.shown = match &self.file {
            None => nr_terminal_attach(view),
            Some(id) => {
                let id = kit::core::c(id);
                // SAFETY: the id is NUL-terminated.
                unsafe { nr_terminal_attach_file(id.as_ptr(), view) }
            }
        };
    }

    /// Sends the core the view, or its new size, once the terminal is placed. Called while
    /// drawing, so the grid changes at most once a frame.
    fn sync(&mut self, window: &mut Window) {
        if !self.enabled || self.frame.size.width <= px(1.0) || self.frame.size.height <= px(1.0) {
            return;
        }
        let scale = window.scale_factor();
        let pixels = |length: Pixels| (f32::from(length) * scale).max(1.0) as u32;
        let view = NrTerminalView {
            width: pixels(self.frame.size.width),
            height: pixels(self.frame.size.height),
            cell: self.cell(window),
        };
        match self.attached {
            Some(was) if was == view => {}
            Some(_) => nr_terminal_resize(self.shown, view),
            None => self.attach(view),
        }
        self.attached = Some(view);
    }

    /// Draws the core's frame of the grid at `bounds`, and has the window drawn again when the
    /// cursor's blink turns. The theme background shows until the session has a frame.
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let mut lent: NrTerminalFrame = kit::core::zeroed();
        // SAFETY: storage for one frame.
        let drawn = self.attached.is_some() && unsafe { nr_terminal_frame(self.shown, &mut lent) };
        if !drawn {
            kit::draw::fill(
                window,
                bounds,
                kit::theme::color(kit::theme::Token::Background),
            );
            return;
        }
        kit::draw::fill(window, bounds, kit::theme::shown(&lent.background));
        let cell = self.cell(window);
        // SAFETY: the core lends `count` pieces until its next `nr_terminal_frame`.
        let pieces = unsafe { std::slice::from_raw_parts(lent.pieces, lent.count) };
        let text = kit::core::text(&lent.text);
        let placed = Placed {
            origin: bounds.origin,
            scale: window.scale_factor(),
            cell_width: cell.width,
        };
        for piece in pieces {
            match piece.kind {
                NrTerminalPieceKind::Fill => placed.fill(piece, window),
                NrTerminalPieceKind::Text => placed.text(piece, &text, window, cx),
            }
        }
        self.blink_after(lent.redraw_after_ms, cx);
    }

    /// Draws again in `milliseconds` for the cursor's blink; 0 for no redraw.
    fn blink_after(&mut self, milliseconds: u32, cx: &mut Context<Self>) {
        if milliseconds == 0 {
            self.blink = None;
            return;
        }
        let wait = Duration::from_millis(u64::from(milliseconds));
        self.blink = Some(cx.spawn(async move |terminal, cx| {
            cx.background_executor().timer(wait).await;
            let _ = terminal.update(cx, |_, cx| cx.notify());
        }));
    }

    /// The terminal is focused when it has the keys and its window is the key window.
    fn sync_focus(&mut self, window: &Window) {
        if self.attached.is_none() {
            return;
        }
        let focused = self.focus.is_focused(window) && window.is_window_active();
        if self.focused != Some(focused) {
            self.focused = Some(focused);
            nr_terminal_focus(self.shown, focused);
        }
    }

    // MARK: Keys

    fn send(&self, text: &str) {
        // SAFETY: `text`'s bytes.
        unsafe { nr_terminal_input(self.shown, text.as_ptr(), text.len()) };
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let held = keystroke.modifiers;
        // The terminal handles clipboard commands: ⌘C, ⌘V and ⌘A on macOS; Control-Shift-C,
        // -V and -A elsewhere, as Windows and Linux terminals do (Control-C goes to the
        // program).
        if clipboard_held(&held) && self.clipboard_command(&keystroke.key, cx) {
            cx.stop_propagation();
            return;
        }
        // ⌘ belongs to the menu bar; the Windows or Super key to the system.
        if held.platform {
            return;
        }
        // While composing, every key goes to the input method.
        if !self.marked.is_empty() {
            return;
        }
        // The core sends keys with no character (except Space) as themselves.
        if nr_terminal_key(self.shown, kit::keys::press(keystroke)) {
            cx.stop_propagation();
            return;
        }
        // Control, or Option as Meta, with a character: the core sends the key
        // (`terminal::input::held_character`).
        if held.control || held.alt {
            let typed = kit::core::c(keystroke.key_char.as_deref().unwrap_or(""));
            let plain = kit::core::c(&plain(keystroke));
            let layout = kit::core::c(&keyboard_layout());
            // SAFETY: NUL-terminated strings.
            if unsafe {
                nr_terminal_held_key(
                    self.shown,
                    layout.as_ptr(),
                    typed.as_ptr(),
                    plain.as_ptr(),
                    kit::keys::held(&held),
                )
            } {
                cx.stop_propagation();
            }
        }
        // Everything else is text, composed by the input system (`replace_text_in_range`).
    }

    /// Runs the clipboard command for `key` (`c`, `v` or `a`). Returns whether `key` was one.
    fn clipboard_command(&self, key: &str, cx: &mut Context<Self>) -> bool {
        match key {
            "c" => self.copy_selection(cx),
            "v" => self.paste_clipboard(cx),
            "a" => nr_terminal_select_all(self.shown),
            _ => return false,
        }
        true
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        let copied = kit::core::text(&nr_terminal_copy(self.shown));
        if !copied.is_empty() {
            write_clipboard(&copied, cx);
        }
    }

    fn paste_clipboard(&self, cx: &mut Context<Self>) {
        if let Some(pasted) = read_clipboard(cx) {
            // SAFETY: the text's bytes.
            unsafe { nr_terminal_paste(self.shown, pasted.as_ptr(), pasted.len()) };
        }
    }

    /// Passes files dropped from outside the app (a screenshot thumbnail, Finder files) to the
    /// program as paths, as terminals do, and focuses the terminal for the text that follows.
    fn drop_files(
        &mut self,
        dropped: &gpui::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths: Vec<String> = dropped
            .paths()
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let paths = kit::core::c(&paths.join("\n"));
        // SAFETY: a NUL-terminated list.
        if unsafe { nr_terminal_drop(self.shown, paths.as_ptr()) } {
            window.focus(&self.focus, cx);
        }
    }

    /// Passes drive files dragged from the window and dropped on the terminal to the program the
    /// same way. True when a drag ended here.
    fn drop_carried(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(drag) = chip::end() else {
            return false;
        };
        let ids = kit::core::c(&drag.ids.join("\n"));
        // SAFETY: a NUL-terminated list.
        if unsafe { nr_terminal_refer(self.shown, ids.as_ptr()) } {
            window.focus(&self.focus, cx);
        }
        window.refresh();
        true
    }

    fn compose(&self, text: &str) {
        if self.attached.is_some() {
            // SAFETY: the text's bytes.
            unsafe { nr_terminal_compose(self.shown, text.as_ptr(), text.len()) };
        }
    }

    fn unmark(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if !self.marked.is_empty() {
            self.marked.clear();
            self.compose("");
        }
    }

    // MARK: Mouse and wheel

    /// Converts a window point to layer pixels from the top-left, as the core expects.
    fn pixel(&self, at: Point<Pixels>, window: &Window) -> (f32, f32) {
        let scale = window.scale_factor();
        (
            f32::from(at.x - self.frame.origin.x) * scale,
            f32::from(at.y - self.frame.origin.y) * scale,
        )
    }

    fn mouse(
        &self,
        action: NrMouseAction,
        button: MouseButton,
        at: Point<Pixels>,
        clicks: usize,
        held: &Modifiers,
        window: &Window,
    ) {
        let button = match button {
            MouseButton::Right => NrMouseButton::Right,
            MouseButton::Middle => NrMouseButton::Middle,
            _ => NrMouseButton::Left,
        };
        let (x, y) = self.pixel(at, window);
        nr_terminal_mouse(
            self.shown,
            action,
            button,
            x,
            y,
            clicks.max(1) as u32,
            kit::keys::held(held),
        );
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.held = Some(event.button);
        self.mouse(
            NrMouseAction::Pressed,
            event.button,
            event.position,
            event.click_count,
            &event.modifiers,
            window,
        );
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &Window) {
        if let (Some(button), Some(pressed)) = (self.held, event.pressed_button)
            && button == pressed
        {
            self.mouse(
                NrMouseAction::Dragged,
                button,
                event.position,
                1,
                &event.modifiers,
                window,
            );
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &Window) {
        if self.held.take() == Some(event.button) {
            self.mouse(
                NrMouseAction::Released,
                event.button,
                event.position,
                event.click_count,
                &event.modifiers,
                window,
            );
        }
    }

    fn wheel(&self, event: &ScrollEvent, window: &Window) {
        let (x, y) = self.pixel(event.position, window);
        // A trackpad reports points; a wheel reports notches.
        let (delta, precise) = match event.delta {
            ScrollDelta::Pixels(delta) => (f64::from(f32::from(delta.y)), true),
            ScrollDelta::Lines(delta) => (f64::from(delta.y), false),
        };
        nr_terminal_wheel(
            self.shown,
            delta,
            precise,
            f64::from(window.scale_factor()),
            x,
            y,
            kit::keys::held(&event.modifiers),
        );
    }

    /// Registers pointer handlers: presses and wheel over the terminal, and drags it started
    /// anywhere in the window.
    /// A button released anywhere in the window: drive files carried here and released over the
    /// terminal (`over`) are dropped on it; otherwise it ends the terminal's press.
    fn button_up(
        &mut self,
        event: &MouseUpEvent,
        over: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dropped = event.button == MouseButton::Left && over && self.drop_carried(window, cx);
        if !dropped {
            self.mouse_up(event, window);
        }
    }

    fn listen(&self, hitbox: &gpui::Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let (terminal, over) = (cx.entity(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                terminal.update(cx, |terminal, cx| terminal.mouse_down(event, window, cx));
            }
        });
        let terminal = cx.entity();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                terminal.update(cx, |terminal, _| terminal.mouse_move(event, window));
            }
        });
        let (terminal, over) = (cx.entity(), hitbox.clone());
        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let over = over.is_hovered(window);
                terminal.update(cx, |terminal, cx| {
                    terminal.button_up(event, over, window, cx)
                });
            }
        });
        let (terminal, over) = (cx.entity(), hitbox.clone());
        window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && over.is_hovered(window) {
                terminal.read(cx).wheel(event, window);
            }
        });
    }
}

impl Drop for Terminal {
    /// The session goes off screen with its view, unless another screen (another tab showing
    /// the same file) has taken it over.
    fn drop(&mut self) {
        nr_terminal_detach(self.shown);
    }
}

/// Whether the clipboard-command modifiers are held: ⌘ alone on macOS, Control and Shift
/// elsewhere.
fn clipboard_held(held: &Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        held.platform && !held.shift && !held.alt && !held.control
    } else {
        held.control && held.shift && !held.alt && !held.platform
    }
}

/// Called on the core's threads when a terminal has something new to draw; redraws the window.
extern "C" fn frame_changed() {
    kit::wake::wake();
}

// MARK: Drawing

/// Where a frame's pieces land: the view's origin in the window, and the core's pixels per
/// point.
struct Placed {
    origin: Point<Pixels>,
    scale: f32,
    /// A cell's width in pixels, where each glyph of a run is set.
    cell_width: f32,
}

impl Placed {
    fn at(&self, x: f32, y: f32) -> Point<Pixels> {
        self.origin + point(px(x / self.scale), px(y / self.scale))
    }

    fn fill(&self, piece: &NrTerminalPiece, window: &mut Window) {
        let bounds = Bounds {
            origin: self.at(piece.x, piece.y),
            size: size(px(piece.width / self.scale), px(piece.height / self.scale)),
        };
        kit::draw::fill(window, bounds, kit::theme::shown(&piece.color));
    }

    /// Sets a run in the terminal font with each glyph on its cell, as Zed's terminal does.
    fn text(&self, piece: &NrTerminalPiece, text: &str, window: &mut Window, cx: &mut App) {
        let Some(run) = text.get(piece.text_from..piece.text_from + piece.text_length) else {
            return;
        };
        let style = TextRun {
            len: run.len(),
            font: terminal_font(piece.bold, piece.italic),
            color: kit::theme::shown(&piece.color),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(
            SharedString::from(run.to_owned()),
            px(nr_terminal_font_points()),
            &[style],
            Some(px(self.cell_width / self.scale)),
        );
        let origin = self.at(piece.x, piece.y);
        let line_height = px(piece.height / self.scale);
        if let Err(error) = line.paint(origin, line_height, TextAlign::Left, None, window, cx) {
            eprintln!("gpui\ta terminal line was not drawn: {error}");
        }
    }
}

/// The terminal font's cell at `scale` pixels per point, from GPUI's metrics of its regular
/// face, fitted to whole pixels by the core (`nr_terminal_cell`).
fn measure_cell(window: &Window, scale: f32) -> NrTerminalCell {
    let text = window.text_system();
    let font = text.resolve_font(&terminal_font(false, false));
    let points = px(nr_terminal_font_points());
    let advance = text
        .advance(font, points, 'm')
        .map_or(points * 0.6, |advance| advance.width);
    let ascent = f32::from(text.ascent(font, points));
    let descent = f32::from(text.descent(font, points)).abs();
    nr_terminal_cell(
        ascent * scale,
        descent * scale,
        f32::from(advance) * scale,
        scale,
    )
}

/// The app's packaged monospaced face, for a system with none of the terminal's families.
const PACKAGED_FAMILY: &str = "Geist Mono";

/// The families the terminal is set in, in preference order: the platform terminal's own, then
/// the app's packaged monospace. GPUI takes the first that is installed.
fn terminal_families() -> &'static [String] {
    static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();
    FAMILIES.get_or_init(|| {
        platform_families()
            .into_iter()
            .chain([PACKAGED_FAMILY.to_owned()])
            .collect()
    })
}

/// SF Mono, the Mac terminal's face (`install_faces`), else Menlo.
#[cfg(target_os = "macos")]
fn platform_families() -> Vec<String> {
    vec!["SF Mono".to_owned(), "Menlo".to_owned()]
}

#[cfg(windows)]
fn platform_families() -> Vec<String> {
    vec!["Cascadia Mono".to_owned(), "Consolas".to_owned()]
}

/// fontconfig's monospace family (what a Linux desktop terminal uses), then the monospaced
/// families most distributions install.
#[cfg(target_os = "linux")]
fn platform_families() -> Vec<String> {
    let configured = std::process::Command::new("fc-match")
        .args(["-f", "%{family[0]}", "monospace"])
        .output()
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|family| !family.is_empty());
    configured
        .into_iter()
        .chain(
            [
                "DejaVu Sans Mono",
                "Liberation Mono",
                "Noto Sans Mono",
                "Ubuntu Mono",
            ]
            .map(str::to_owned),
        )
        .collect()
}

/// The terminal font in one of its four faces.
fn terminal_font(bold: bool, italic: bool) -> Font {
    let families = terminal_families();
    Font {
        family: SharedString::from(families[0].clone()),
        features: FontFeatures::disable_ligatures(),
        fallbacks: Some(FontFallbacks(Arc::new(families[1..].to_vec()))),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

/// Where macOS keeps SF Mono's static faces: Terminal.app's own copies. The system's variable
/// SF Mono is a hidden face GPUI can't choose a weight of.
#[cfg(target_os = "macos")]
const SF_MONO_FACES: [&str; 4] = [
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts/SF-Mono-Regular.otf",
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts/SF-Mono-Bold.otf",
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts/SF-Mono-RegularItalic.otf",
    "/System/Applications/Utilities/Terminal.app/Contents/Resources/Fonts/SF-Mono-BoldItalic.otf",
];

/// Gives GPUI the terminal's faces that are not installed for every app: SF Mono on a Mac.
/// Elsewhere the platform's terminal families are installed fonts.
pub fn install_faces(cx: &mut App) {
    #[cfg(target_os = "macos")]
    {
        let faces: Vec<std::borrow::Cow<'static, [u8]>> = SF_MONO_FACES
            .iter()
            .filter_map(|path| std::fs::read(path).ok())
            .map(std::borrow::Cow::Owned)
            .collect();
        if faces.len() < SF_MONO_FACES.len() {
            eprintln!("gpui\tSF Mono is missing faces; the terminal falls back to Menlo");
        }
        if let Err(error) = cx.text_system().add_fonts(faces) {
            eprintln!("gpui\tSF Mono was not loaded: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = cx;
}

/// What a key types with Shift alone: its letter, uppercase while Shift is held.
fn plain(keystroke: &Keystroke) -> String {
    if keystroke.modifiers.shift {
        keystroke.key.to_uppercase()
    } else {
        keystroke.key.clone()
    }
}

/// The keyboard layout's id, so the core can tell whether Option plus a letter is Meta on it
/// (`terminal::input::option_is_meta`).
#[cfg(target_os = "macos")]
fn keyboard_layout() -> String {
    use std::ffi::{c_char, c_void};
    type CFTypeRef = *const c_void;
    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> CFTypeRef;
        fn TISGetInputSourceProperty(source: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
        static kTISPropertyInputSourceID: CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: CFTypeRef);
        fn CFStringGetCString(
            text: CFTypeRef,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> bool;
    }
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: plain Carbon calls. The source is released, and its id belongs to it.
    unsafe {
        let source = TISCopyCurrentKeyboardLayoutInputSource();
        if source.is_null() {
            return String::new();
        }
        let id = TISGetInputSourceProperty(source, kTISPropertyInputSourceID);
        let mut buffer = [0 as c_char; 256];
        let read = !id.is_null()
            && CFStringGetCString(id, buffer.as_mut_ptr(), buffer.len() as isize, UTF8);
        CFRelease(source);
        if read {
            std::ffi::CStr::from_ptr(buffer.as_ptr())
                .to_string_lossy()
                .into_owned()
        } else {
            String::new()
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keyboard_layout() -> String {
    String::new()
}

impl EventEmitter<Ask> for Terminal {}

impl Screen for Terminal {
    fn reload(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }

    fn keys(&self, _: &App) -> Option<FocusHandle> {
        Some(self.focus.clone())
    }

    fn probe(&self, probe: &mut Probe, window: &Window, _: &App) {
        if self.focus.is_focused(window) {
            probe.keys = "TerminalView".to_owned();
        }
    }
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let at = self.marked.encode_utf16().count();
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        (!self.marked.is_empty()).then(|| 0..self.marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.unmark(window, cx);
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unmark(window, cx);
        self.send(text);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.marked = text.to_owned();
        self.compose(text);
    }

    /// Places the input method's window under the terminal cursor.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let mut rect = [0.0_f32; 4];
        // SAFETY: storage for four numbers.
        if !unsafe { nr_terminal_cursor_rect(self.shown, rect.as_mut_ptr()) } {
            return None;
        }
        let scale = window.scale_factor();
        Some(Bounds {
            origin: point(
                self.frame.origin.x + px(rect[0] / scale),
                self.frame.origin.y + px(rect[1] / scale),
            ),
            size: size(px(rect[2] / scale), px(rect[3] / scale)),
        })
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

impl Render for Terminal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal = cx.entity();
        let frame = self.frame;
        kit::controls::placed("terminal", cx.entity_id(), frame)
            .track_focus(&self.focus)
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(Self::drop_files))
            .cursor(CursorStyle::IBeam)
            .child(
                canvas(
                    move |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, cx| {
                        terminal.update(cx, |screen, cx| {
                            screen.sync(window);
                            screen.sync_focus(window);
                            screen.listen(&hitbox, window, cx);
                            screen.paint(bounds, window, cx);
                        });
                        let focus = terminal.read(cx).focus.clone();
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(bounds, terminal.clone()),
                            cx,
                        );
                    },
                )
                .size_full(),
            )
    }
}

// MARK: Checks

pub fn register(table: &mut Table) {
    table.answer(Route::Kind(NrScriptKind::TerminalTyping), typing);
}

/// macOS key codes for the characterless keys a check presses, by their GPUI names.
const NAMED_CODES: [(u16, &str); 15] = [
    (36, "enter"),
    (76, "enter"),
    (48, "tab"),
    (49, "space"),
    (51, "backspace"),
    (53, "escape"),
    (117, "delete"),
    (123, "left"),
    (124, "right"),
    (125, "down"),
    (126, "up"),
    (115, "home"),
    (119, "end"),
    (116, "pageup"),
    (121, "pagedown"),
];

/// `termtype`: sends `NR_TERM_TYPE`'s presses to the focused view as typed keys, through the
/// input system.
fn typing(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    let mut index = 0;
    loop {
        let mut press: NrScriptPress = kit::core::zeroed();
        // SAFETY: storage for one press.
        if !unsafe { nr_script_press(index, &mut press) } {
            break;
        }
        index += 1;
        let held = press.modifiers;
        let modifiers = Modifiers {
            platform: held & NR_HELD_COMMAND != 0,
            control: held & NR_HELD_CONTROL != 0,
            alt: held & NR_HELD_OPTION != 0,
            shift: held & NR_HELD_SHIFT != 0,
            function: false,
        };
        let characters = kit::core::text(&press.characters);
        let plain = kit::core::text(&press.plain);
        let named = NAMED_CODES
            .iter()
            .find(|(code, _)| *code == press.code)
            .map(|(_, name)| (*name).to_owned());
        let keystroke = Keystroke {
            modifiers,
            key: named.clone().unwrap_or_else(|| plain.to_lowercase()),
            key_char: (named.is_none() || named.as_deref() == Some("space"))
                .then_some(characters)
                .filter(|typed| !typed.is_empty()),
        };
        window.draw(cx).clear(cx);
        window.dispatch_keystroke(keystroke, cx);
    }
    Answer::Done
}
