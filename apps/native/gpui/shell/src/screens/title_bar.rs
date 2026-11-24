//! The title bar: back and forward, the open tabs, the new-tab button, the review button (while
//! files wait for review), the sessions button and the update button, laid out by the core
//! (`nr_title_bar_lay_out`). In settings it shows the
//! settings title instead, which Back leaves.
//!
//! A press asks the core what is under it (`nr_title_bar_hit`); a press on the bar itself moves
//! the window. A middle click closes the tab it releases on (`nr_title_bar_middle_up`), a right
//! click opens a tab's menu (`menus::ffi::menu_tab`), and a horizontal scroll moves the tab row
//! when the tabs overflow (`nr_title_bar_scroll`). Dragging a tab moves it along the row and
//! reorders the tabs (`nr_title_bar_hold`, `nr_title_bar_carry`, `nr_title_bar_let_go`).
//!
//! Controls show tooltips (`nr_title_bar_tooltip`). Back, forward, "+", the review button and the
//! update button fade their background on hover (`nr_hover_level`). A hovered tab shows its close button over
//! the end of its title, which fades out before it the way an overlong title fades at the tab's
//! edge; the core says when and where (`nr_title_bar_tab`).

use super::{Ask, Screen, on_bubble, on_bubble_over};
use crate::script::{Answer, Pointer, Route, Step, Table, workspace};
use gpui::{
    App, Bounds, Context, Corners, EventEmitter, Hitbox, HitboxBehavior, Hsla, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollDelta, ScrollEvent, Styled, Window, canvas, point, px,
};
use kit::controls::spinner;
use kit::core::{c, contains, frame, inset, offset, rect, text, zeroed};
use kit::draw::{fill, oval, rounded};
use kit::icons::{self, Glyph};
use kit::nr::*;
use kit::text::{draw_fading_line, draw_line, width};
use kit::theme::{self, CONTROL_RADIUS, Face, Token, color, mixed};
use northroom_core::window::title_bar::WindowButtons;
use std::ffi::CStr;
use std::rc::Rc;

/// What the bar shows, read from the core on change rather than every frame. A tab's title is
/// read through the drive (`Workspace::path`), which costs about 1 ms on a 5,000-file folder.
#[derive(Default)]
struct Bar {
    titles: Vec<String>,
    /// Each title's width in the tab face, measured once the titles are read.
    measured: Vec<f64>,
    glyphs: Vec<Option<Glyph>>,
    /// The agent each tab's terminal runs and its activity (`nr_tab_terminal`).
    terminals: Vec<TabTerminal>,
    active: usize,
    back: bool,
    forward: bool,
    settings: String,
    /// The update button's label (`nr_update_button_label`); empty while it is hidden.
    update: String,
    /// The update is downloading, so the button shows a spinner and ignores presses.
    downloading: bool,
    /// The sessions button's content (`sessions_button`).
    sessions: SessionsButton,
    /// Files wait for review, so the review button shows (`nr_changes_waiting`).
    review: bool,
}

impl Bar {
    fn read() -> Self {
        let words = nr_title_bar_words();
        // With no drive chosen the bar has no tabs but still shows its controls.
        let count = if nr_drive_ready() { nr_tabs_count() } else { 0 };
        let titles: Vec<String> = (0..count).map(|index| text(&nr_tab_title(index))).collect();
        Self {
            measured: titles
                .iter()
                .map(|title| f64::from(width(title, title_face())))
                .collect(),
            titles,
            glyphs: (0..count)
                .map(|index| icons::named(&text(&nr_tab_glyph(index))))
                .collect(),
            terminals: (0..count).map(TabTerminal::read).collect(),
            active: nr_tabs_active(),
            back: nr_history_can_go_back(),
            forward: nr_history_can_go_forward(),
            settings: text(&words.settings),
            update: text(&nr_update_button_label()),
            downloading: nr_update_button_downloading(),
            sessions: sessions_button(),
            review: nr_changes_waiting() > 0,
        }
    }
}

/// What a terminal tab shows: its program's logo name (empty for none) and its session's
/// activity number.
struct TabTerminal {
    logo: String,
    activity: u32,
}

impl TabTerminal {
    fn read(index: usize) -> Self {
        let terminal = nr_tab_terminal(index);
        Self {
            logo: text(&terminal.logo),
            activity: terminal.activity,
        }
    }
}

/// The core's last layout of the bar and each tab.
struct Laid {
    bar: NrTitleBar,
    tabs: Vec<NrTitleBarTab>,
}

impl Default for Laid {
    fn default() -> Self {
        Self {
            bar: zeroed(),
            tabs: Vec::new(),
        }
    }
}

pub struct TitleBar {
    bar: Bar,
    laid: Laid,
    frame: Bounds<Pixels>,
    /// The pointer's window position while it is over the bar.
    pointer: Option<Point<Pixels>>,
}

impl TitleBar {
    pub fn new(_: &mut Window, _: &mut Context<Self>) -> Self {
        Self {
            bar: Bar::default(),
            laid: Laid::default(),
            frame: Bounds::default(),
            pointer: None,
        }
    }

    /// Converts `at` (window points) to the core's bar coordinates.
    fn local(&self, at: Point<Pixels>) -> (f64, f64) {
        (
            f64::from(at.x - self.frame.origin.x),
            f64::from(at.y - self.frame.origin.y),
        )
    }

    /// Converts a bar frame to a window frame.
    fn in_window(&self, frame: &NrRect) -> Bounds<Pixels> {
        offset(
            rect(frame),
            f32::from(self.frame.origin.x),
            f32::from(self.frame.origin.y),
        )
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(Ask::Tooltip(None));
        let (x, y) = self.local(event.position);
        if event.button == MouseButton::Right
            || (event.button == MouseButton::Left && event.modifiers.control)
        {
            self.tab_menu(event.position, x, y, cx);
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let hit = nr_title_bar_hit(x, y);
        match hit.control {
            NrBarControl::Back => nr_history_back(),
            NrBarControl::Forward => nr_history_forward(),
            NrBarControl::Close => nr_tab_close(hit.tab as usize),
            NrBarControl::Plus => nr_tab_open(),
            NrBarControl::Tab => {
                nr_tab_select(hit.tab as usize);
                nr_title_bar_hold(hit.tab as usize, x, y);
            }
            NrBarControl::Update => {
                // Opens the update dialog: download the update, or restart into it.
                if !self.bar.downloading {
                    crate::updates::request(true, cx);
                }
                return;
            }
            NrBarControl::Sessions => {
                self.sessions_menu(cx);
                return;
            }
            NrBarControl::Review => nr_changes_open_current(),
            // Anywhere else on the bar moves the window, and a double click does whatever the
            // system title-bar setting says.
            NrBarControl::Bar => {
                if event.click_count == 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
                return;
            }
        }
        cx.emit(Ask::Changed);
    }

    /// The pointer moved with the left button down, so the held tab, if any, follows it. Returns
    /// whether a tab is being dragged.
    fn carry(&mut self, at: Point<Pixels>, window: &Window, cx: &mut Context<Self>) -> bool {
        let (x, y) = self.local(at);
        match nr_title_bar_carry(x, y, f64::from(window.scale_factor())) {
            NrBarCarry::Still => return false,
            NrBarCarry::Moved => self.bar = Bar::read(),
            NrBarCarry::Carried => {}
        }
        self.pointer = Some(at);
        cx.emit(Ask::Tooltip(None));
        cx.notify();
        true
    }

    /// The left button was released anywhere in the window; a dragged tab settles in place.
    fn let_go(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button == MouseButton::Left && nr_title_bar_let_go() {
            cx.emit(Ask::Changed);
        }
    }

    /// The middle button was released; the core decides what happens to the tab under it.
    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let (x, y) = self.local(event.position);
        if event.button == MouseButton::Middle && nr_title_bar_middle_up(x, y) {
            cx.emit(Ask::Tooltip(None));
            cx.emit(Ask::Changed);
        }
    }

    /// Opens a tab's menu at the pointer on right click.
    fn tab_menu(&self, at: Point<Pixels>, x: f64, y: f64, cx: &mut Context<Self>) {
        let again: kit::menu::Again = Rc::new(move |_| menu_tab(x, y));
        cx.emit(Ask::Menu {
            which: NrMenu::Tab,
            again,
            at,
            below: false,
        });
    }

    /// The sessions menu, listing every running session by its prompt. The core rebuilds it on
    /// each read, so it stays current while open. It hangs from the button, right edges aligned.
    fn sessions_menu(&self, cx: &mut Context<Self>) {
        let button = self.in_window(&self.laid.bar.sessions);
        let at = point(
            button.origin.x + button.size.width - px(menu_sessions().min_width as f32),
            button.origin.y + button.size.height + px(4.0),
        );
        let again: kit::menu::Again = Rc::new(|_| menu_sessions());
        cx.emit(Ask::Menu {
            which: NrMenu::Sessions,
            again,
            at,
            below: true,
        });
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        over: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button == Some(MouseButton::Left) && self.carry(event.position, window, cx)
        {
            return;
        }
        let pointer = over.then_some(event.position);
        if pointer == self.pointer {
            return;
        }
        self.pointer = pointer;
        cx.notify();
        let Some(at) = pointer else {
            cx.emit(Ask::Tooltip(None));
            return;
        };
        // The core's tooltip for the control under the resting pointer.
        let (x, y) = self.local(at);
        let mut frame = kit::core::empty_rect();
        let update = c(&self.bar.update);
        // SAFETY: a NUL-terminated name and a rect to fill.
        let words = text(&unsafe { nr_title_bar_tooltip(x, y, update.as_ptr(), &mut frame) });
        if words.is_empty() {
            cx.emit(Ask::Tooltip(None));
        } else {
            cx.emit(Ask::Tooltip(Some((self.in_window(&frame), words))));
        }
    }

    /// Scrolls the tab row horizontally when the tabs overflow, using only the horizontal
    /// travel.
    fn scroll(&mut self, event: &ScrollEvent, cx: &mut Context<Self>) {
        let (across, unit) = match event.delta {
            ScrollDelta::Pixels(delta) => (-f64::from(delta.x), NR_WHEEL_POINTS),
            ScrollDelta::Lines(delta) => (-f64::from(delta.x), NR_WHEEL_CLICKS),
        };
        let (x, y) = self.local(event.position);
        match nr_title_bar_scroll(x, y, across, unit) {
            NrBarScroll::Moved => {
                cx.stop_propagation();
                cx.notify();
            }
            NrBarScroll::Kept => cx.stop_propagation(),
            NrBarScroll::PassOn => {}
        }
    }

    fn listen(&self, hitbox: &Hitbox, window: &mut Window, cx: &mut Context<Self>) {
        let entity = cx.entity();
        let bar = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseDownEvent, window, cx| {
            bar.update(cx, |bar, cx| bar.mouse_down(event, window, cx));
        });
        let bar = entity.clone();
        on_bubble_over(window, hitbox, move |event: &MouseUpEvent, _, cx| {
            bar.update(cx, |bar, cx| bar.mouse_up(event, cx));
        });
        let bar = entity.clone();
        on_bubble(window, move |event: &MouseUpEvent, _, cx| {
            bar.update(cx, |bar, cx| bar.let_go(event, cx));
        });
        let bar = entity.clone();
        on_bubble_over(window, hitbox, move |event: &ScrollEvent, _, cx| {
            bar.update(cx, |bar, cx| bar.scroll(event, cx));
        });
        let (bar, over) = (entity, hitbox.clone());
        on_bubble(window, move |event: &MouseMoveEvent, window, cx| {
            let hovered = over.is_hovered(window);
            bar.update(cx, |bar, cx| bar.mouse_move(event, hovered, window, cx));
        });
    }

    /// The tab row's scroll offset and each tab's width as last drawn, for a check.
    fn said_tabs(&self) -> String {
        let widths: Vec<String> = self
            .laid
            .tabs
            .iter()
            .map(|tab| format!("{:.1}", tab.frame.width))
            .collect();
        format!(
            "tabs\toffset {} shown {}\twidths {}",
            self.laid.bar.offset.round() as i64,
            self.laid.bar.tabs.width.round() as i64,
            widths.join(" ")
        )
    }
}

impl EventEmitter<Ask> for TitleBar {}

impl Screen for TitleBar {
    fn reload(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.bar = Bar::read();
        cx.notify();
    }

    fn place(&mut self, frame: Bounds<Pixels>, _: &NrWindowLayout, cx: &mut Context<Self>) {
        if self.frame != frame {
            self.frame = frame;
            cx.notify();
        }
    }
}

impl Render for TitleBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bar = cx.entity();
        kit::controls::placed("title-bar", cx.entity_id(), self.frame).child(
            canvas(
                |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                move |_, hitbox, window, cx| {
                    bar.update(cx, |bar, cx| {
                        bar.laid = paint(window, cx, &bar.bar, bar.frame, bar.pointer);
                        bar.listen(&hitbox, window, cx);
                    })
                },
            )
            .size_full(),
        )
    }
}

/// The hover level of button `key` now, given whether the pointer is over it. While a fade is
/// in progress the window keeps redrawing until every button settles.
fn fade(window: &mut Window, key: &CStr, on: bool) -> f32 {
    let now = kit::motion::now();
    // SAFETY: a NUL-terminated key.
    let level = unsafe { nr_hover_level(key.as_ptr(), on, now) };
    if !nr_hover_settled(now) {
        window.request_animation_frame();
    }
    level.level as f32
}

/// The face for tab titles and the settings title.
fn title_face() -> Face {
    Face::sans(12.0, 500.0)
}

/// The face for the sessions button's text, same as a tab title.
fn pill_face() -> Face {
    Face::sans(12.0, 500.0)
}

/// The bar's window frame and the pointer position, if any, which each part needs to place
/// itself and show hover.
struct Placed {
    frame: Bounds<Pixels>,
    pointer: Option<Point<Pixels>>,
}

impl Placed {
    /// Converts a core bar rect to window coordinates.
    fn at(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        offset(
            bounds,
            f32::from(self.frame.origin.x),
            f32::from(self.frame.origin.y),
        )
    }

    /// Whether the pointer is over a core-laid-out rect.
    fn lit(&self, bounds: Bounds<Pixels>) -> bool {
        self.pointer
            .is_some_and(|pointer| contains(&self.at(bounds), pointer))
    }

    /// Fills a control's background with the text colour at opacity `strength`.
    fn light(&self, window: &mut Window, bounds: Bounds<Pixels>, strength: f32) {
        rounded(
            window,
            self.at(bounds),
            mixed(Token::Foreground, strength),
            CONTROL_RADIUS,
        );
    }
}

/// Draws the bar in `frame`, with the pointer's window position if any. Returns the core's
/// layout.
fn paint(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    frame: Bounds<Pixels>,
    pointer: Option<Point<Pixels>>,
) -> Laid {
    let place = Placed { frame, pointer };
    fill(window, frame, color(Token::Frame));
    let measured = &read.measured;
    // The pill grows out of the button while it has text, and shrinks back when it has none.
    let words = &read.sessions.words;
    let grown = fade(window, c"sessions-pill", !words.is_empty());
    // The pointer in the core's bar coordinates; the tab under it shows its close button.
    let pointed = pointer.map(|at| NrPoint {
        x: f64::from(at.x - frame.origin.x),
        y: f64::from(at.y - frame.origin.y),
    });
    // In full screen the window buttons are hidden and the bar reclaims their space.
    let window_buttons = match window.is_fullscreen() {
        true => WindowButtons::Hidden,
        false => WindowButtons::Shown,
    };
    // SAFETY: `measured` holds one width per tab, and `pointed` one point or none.
    let bar = unsafe {
        nr_title_bar_lay_out(
            f64::from(frame.size.width),
            measured.as_ptr(),
            measured.len(),
            !read.update.is_empty(),
            f64::from(width(words, pill_face())),
            f64::from(grown),
            read.review,
            pointed
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            window_buttons,
        )
    };
    let tabs: Vec<NrTitleBarTab> = (0..bar.tab_count)
        .map(|index| {
            let mut tab: NrTitleBarTab = zeroed();
            // SAFETY: a tab to fill.
            unsafe { nr_title_bar_tab(index, &mut tab) };
            tab
        })
        .collect();
    paint_ways(window, cx, read, &bar, &place);

    if nr_settings_tab() >= 0 {
        // In settings the bar shows their title, and Back leaves them.
        draw_line(
            window,
            cx,
            &read.settings,
            title_face(),
            color(Token::Foreground),
            f32::from(frame.origin.x) + bar.settings_x as f32,
            f32::from(frame.origin.y) + 13.0,
            16.0,
            240.0,
            None,
        );
        paint_controls(window, cx, read, &bar, &place, grown);
        return Laid { bar, tabs };
    }
    let tabs_frame = rect(&bar.tabs);
    let clip = bar.clipped.then(|| gpui::ContentMask {
        bounds: place.at(tabs_frame),
    });
    // A dragged tab draws last, on the bar background, over the tabs it passes.
    let carried = usize::try_from(bar.carried).ok();
    window.with_content_mask(clip, |window| {
        let others = (0..tabs.len()).filter(|&index| Some(index) != carried);
        for index in others.chain(carried) {
            let tab = &tabs[index];
            if Some(index) == carried {
                let ground = place.at(rect(&tab.frame));
                rounded(window, ground, color(Token::Frame), CONTROL_RADIUS);
            }
            paint_tab(window, cx, read, &place, index, tab, tabs_frame);
        }
    });
    let plus = rect(&bar.plus);
    let plus_level = fade(window, c"plus", place.lit(plus));
    place.light(window, plus, 0.08 * plus_level);
    icons::draw(
        window,
        cx,
        Glyph::Plus,
        place.at(kit::core::inset(plus, 6.0, 6.0)),
        color(Token::Foreground),
    );
    paint_controls(window, cx, read, &bar, &place, grown);
    Laid { bar, tabs }
}

/// Paints back and forward. Each highlights on hover when enabled and draws its arrow at 50%
/// opacity when not.
fn paint_ways(window: &mut Window, cx: &mut App, read: &Bar, bar: &NrTitleBar, place: &Placed) {
    let text_color = color(Token::Foreground);
    let (back, forward) = (rect(&bar.back), rect(&bar.forward));
    let back_level = fade(window, c"back", read.back && place.lit(back));
    place.light(window, back, 0.08 * back_level);
    let forward_level = fade(window, c"forward", read.forward && place.lit(forward));
    place.light(window, forward, 0.08 * forward_level);
    // A disabled arrow draws at 50% opacity.
    let half = |open: bool| {
        if open {
            text_color
        } else {
            theme::faded(text_color, 0.5)
        }
    };
    icons::draw(
        window,
        cx,
        Glyph::ArrowLeft,
        place.at(kit::core::inset(back, 6.0, 6.0)),
        half(read.back),
    );
    icons::draw(
        window,
        cx,
        Glyph::ArrowRight,
        place.at(kit::core::inset(forward, 6.0, 6.0)),
        half(read.forward),
    );
}

/// Paints tab `index` where the core placed it in `tabs_frame`: its background when active or
/// hovered, its glyph, its title faded where the core says it doesn't fit, and its close button
/// over the end of the title when the core shows it.
fn paint_tab(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    place: &Placed,
    index: usize,
    placed: &NrTitleBarTab,
    tabs_frame: Bounds<Pixels>,
) {
    let tab = rect(&placed.frame);
    let is_active = index == read.active;
    let hovered = place.lit(tab) && place.lit(tabs_frame);
    if is_active {
        place.light(window, tab, 0.08);
    } else if hovered {
        // An inactive tab fills at 5% and uses the text colour while hovered.
        place.light(window, tab, 0.05);
    }
    let shade = if is_active || hovered {
        color(Token::Foreground)
    } else {
        color(Token::MutedForeground)
    };
    paint_tab_mark(
        window,
        cx,
        read,
        index,
        place.at(rect(&placed.glyph)),
        shade,
    );
    let left = f32::from(place.frame.origin.x);
    let fade = placed
        .fades
        .then_some((left + placed.fade_start as f32)..(left + placed.fade_end as f32));
    draw_fading_line(
        window,
        cx,
        &read.titles[index],
        title_face(),
        shade,
        left + placed.label_x as f32,
        f32::from(place.frame.origin.y) + 13.0,
        16.0,
        fade,
    );
    if placed.close_shown {
        paint_tab_close(window, cx, place, rect(&placed.close));
    }
}

/// Paints the icon at the start of tab `index` in `slot`: its terminal agent's logo, else its
/// glyph, with the session activity dot in the corner, as on its card and row.
fn paint_tab_mark(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    index: usize,
    slot: Bounds<Pixels>,
    shade: Hsla,
) {
    let terminal = &read.terminals[index];
    let pixels = f32::from(slot.size.width) * window.scale_factor();
    match kit::lists::thumbs::logo(&terminal.logo, pixels, cx) {
        Some(logo) => {
            let _ = window.paint_image(slot, slot, Corners::default(), logo, 0, false);
        }
        None => {
            if let Some(glyph) = read.glyphs[index] {
                icons::draw(window, cx, glyph, slot, shade);
            }
        }
    }
    let look = nr_terminal_look(terminal.activity);
    let dot = text(&look.dot);
    if !dot.is_empty() {
        paint_corner_dot(
            window,
            cx,
            slot,
            &text(&look.sign),
            theme::faded(theme::named(&dot), look.dot_fade),
        );
    }
}

/// Draws a 5 pt `sign` dot (`kit::signs`) at the bottom-right of `slot`, ringed in the bar
/// colour so it reads against the mark.
fn paint_corner_dot(window: &mut Window, cx: &App, slot: Bounds<Pixels>, sign: &str, shade: Hsla) {
    let at = frame(
        f32::from(slot.origin.x + slot.size.width) - 4.0,
        f32::from(slot.origin.y + slot.size.height) - 4.0,
        5.0,
        5.0,
    );
    oval(
        window,
        inset(kit::signs::reach(sign, at), -1.5, -1.5),
        color(Token::Frame),
    );
    kit::signs::paint(window, cx, sign, at, shade);
}

/// Paints a tab's close button, highlighted and in the text colour on hover.
fn paint_tab_close(window: &mut Window, cx: &mut App, place: &Placed, close: Bounds<Pixels>) {
    if place.lit(close) {
        place.light(window, close, 0.08);
    }
    let shade = if place.lit(close) {
        color(Token::Foreground)
    } else {
        color(Token::MutedForeground)
    };
    icons::draw(
        window,
        cx,
        Glyph::Close,
        place.at(kit::core::inset(close, 2.0, 2.0)),
        shade,
    );
}

/// Paints the controls at the bar's right end: the review button and the update button when
/// shown, and the sessions button.
fn paint_controls(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    bar: &NrTitleBar,
    place: &Placed,
    grown: f32,
) {
    paint_review(window, cx, rect(&bar.review), place);
    paint_sessions(window, cx, read, bar, place, grown);
    paint_update(window, cx, read, rect(&bar.update), place);
}

/// Paints the review button, left of the sessions button while files wait for review: the
/// Changes tab's diff glyph, as "+" is drawn.
fn paint_review(window: &mut Window, cx: &mut App, button: Bounds<Pixels>, place: &Placed) {
    if kit::core::is_empty(&button) {
        return;
    }
    let level = fade(window, c"review", place.lit(button));
    place.light(window, button, 0.08 * level);
    icons::draw(
        window,
        cx,
        Glyph::Diff,
        place.at(kit::core::inset(button, 6.0, 6.0)),
        color(Token::Foreground),
    );
}

/// Paints the sessions button, left of the update button: a terminal glyph, muted while no
/// session runs, with a corner dot for a finished or working session. While sessions need the
/// person it becomes a pill in that colour showing how many. The pill grows out of the button
/// and back (`grown`, 0 to 1), its background and text fading with it.
fn paint_sessions(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    bar: &NrTitleBar,
    place: &Placed,
    grown: f32,
) {
    let button = rect(&bar.sessions);
    let slot = place.at(rect(&bar.sessions_glyph));
    let said = &read.sessions;
    let ground = theme::faded(color(Token::StatusWarning), f64::from(grown));
    rounded(window, place.at(button), ground, CONTROL_RADIUS);
    let level = fade(window, c"sessions", place.lit(button));
    place.light(window, button, 0.08 * level);
    let resting = if said.quiet {
        Token::MutedForeground
    } else {
        Token::Foreground
    };
    let ink = color(Token::StatusWarningForeground);
    icons::draw(
        window,
        cx,
        Glyph::TerminalWindow,
        slot,
        theme::faded(color(resting), f64::from(1.0 - grown)),
    );
    icons::draw(
        window,
        cx,
        Glyph::TerminalWindow,
        slot,
        theme::faded(ink, f64::from(grown)),
    );
    if let Some(dot) = said.dot {
        paint_corner_dot(
            window,
            cx,
            slot,
            said.sign.name(),
            theme::faded(color(dot), f64::from(1.0 - grown)),
        );
    }
    if said.words.is_empty() {
        return;
    }
    // Clip the text to how far the pill has grown.
    let clip = gpui::ContentMask {
        bounds: place.at(button),
    };
    window.with_content_mask(Some(clip), |window| {
        draw_line(
            window,
            cx,
            &said.words,
            pill_face(),
            theme::faded(ink, f64::from(grown)),
            f32::from(place.frame.origin.x) + bar.sessions_words_x as f32,
            f32::from(place.frame.origin.y) + 13.0,
            16.0,
            240.0,
            None,
        );
    });
}

/// Paints the update button: a ghost icon button in the call-to-action colour showing an arrow
/// in a circle, or a spinner while downloading (when the button is disabled).
fn paint_update(
    window: &mut Window,
    cx: &mut App,
    read: &Bar,
    button: Bounds<Pixels>,
    place: &Placed,
) {
    if kit::core::is_empty(&button) {
        return;
    }
    let level = fade(window, c"update", !read.downloading && place.lit(button));
    place.light(window, button, 0.08 * level);
    let middle = place.at(button).center();
    let glyph = Bounds {
        origin: point(middle.x - px(7.0), middle.y - px(7.0)),
        size: gpui::size(px(14.0), px(14.0)),
    };
    if read.downloading {
        // `animate-spin`: one turn per second, linear.
        spinner::paint(
            window,
            cx,
            glyph,
            Token::Cta,
            kit::motion::now(),
            kit::platform::reduce_motion(),
        );
    } else {
        icons::draw(window, cx, Glyph::ArrowCircleUp, glyph, color(Token::Cta));
    }
}

// MARK: A check's steps

pub fn register(table: &mut Table) {
    table.answer(Route::Event(NrScriptEvent::Tabs), said_tabs);
    table.answer(Route::Kind(NrScriptKind::UpdateChanged), update_changed);
    table.answer(Route::Kind(NrScriptKind::UpdateCheck), update_check);
}

/// `ev:tabs`: the tab row's scroll offset and each tab's width, after a fresh draw.
fn said_tabs(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    window.draw(cx).clear(cx);
    let bar = workspace(window).read(cx).bar.clone();
    eprintln!("{}", bar.read(cx).said_tabs());
    Answer::Done
}

/// The update status changed; the bar's button and the menu bar item follow it.
fn update_changed(_: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    workspace(window).update(cx, |workspace, cx| workspace.changed(window, cx));
    Answer::Done
}

/// `update:check` (`number` 1) or `update:background`.
fn update_check(step: &Step, _: &mut Pointer, window: &mut Window, cx: &mut App) -> Answer {
    workspace(window).update(cx, |_, cx| crate::updates::request(step.number == 1, cx));
    Answer::Done
}
