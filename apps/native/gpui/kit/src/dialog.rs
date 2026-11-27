//! The dialog the core requests (`nr_prompt`), drawn over the window: a 50% ink scrim and a
//! panel in the background colour with a 1 px border and 14 px corners. The panel holds a
//! title, an optional message, an optional one-line field, and Cancel beside the action. The
//! naming prompt has a close mark in its corner; a `destructive` alert uses the warning colour
//! for its action. The pairing code is drawn alone instead: a light card of round dots with a
//! line around it that runs down while the code can be used. It closes by Escape or a press
//! outside it, and by itself once the code is used or stale (`nr_pair_left`).
//!
//! The core owns the layout (`menus::ffi::prompt_layout`), keys and focus (`nr_dialog_key`,
//! `nr_dialog_focus`), whether the action is available (`nr_prompt_takes`) and what answering
//! does (`nr_prompt_answer`). The scrim fades in over 150 ms; the panel fades in and scales up
//! from 95% about its centre over 200 ms.
//!
//! The structure follows GPUI Kit's dialog: one deferred layer at the window origin whose scrim
//! blocks the pointer. It takes focus (its field's, if any) while open and restores it on close.

use crate::controls::Line;
use crate::controls::input::{Command, Field, Ground, Keys};
use crate::controls::{Drawn, Faced, Frame};
use crate::core::{c, contains, inset, text};
use crate::icons::{self, Glyph};
use crate::motion::{Move, Pose};
use crate::theme::{Face, Token, color, mixed};
use gpui::{
    App, Bounds, Context, DispatchPhase, Entity, FocusHandle, Focusable, HitboxBehavior, Hsla,
    IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, PathBuilder, Pixels,
    Point, Render, Rgba, ScrollEvent, Subscription, Task, Window, canvas, div, prelude::*, px,
};
use northroom_core::dialog::layout::{Button, CodePlace, CornerMark, Foot, PromptLayout as Layout};
use northroom_core::ffi::{NrKeyPress, NrMotion};
use northroom_core::menus::ffi::{
    NrDialog, NrDialogIntent, NrEffect, NrMenuEffect, nr_dialog_close, nr_dialog_focus,
    nr_dialog_key, nr_dialog_open, nr_pair_left, nr_prompt, nr_prompt_answer, nr_prompt_takes,
    prompt_layout,
};
use std::rc::Rc;
use std::time::Duration;

// A drawn alert box, used where the platform's alert cannot keep Return on the named button.
#[cfg(not(target_os = "macos"))]
mod alert;
#[cfg(not(target_os = "macos"))]
pub use alert::{AlertBox, Answered};

/// Receives the dialog's answer for the owner to carry out.
pub type Done = Rc<dyn Fn(NrMenuEffect, &mut Window, &mut App)>;

fn place(width: f32, height: f32) -> Layout {
    prompt_layout(f64::from(width), f64::from(height))
}

/// The pressable parts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Cancel,
    Action,
    Close,
    /// Anywhere off a code's card.
    Outside,
}

/// How often the line around a code is drawn again, and the code checked for being used.
const CODE_TICK: Duration = Duration::from_millis(100);

/// The open dialog.
struct Open {
    /// The core's dialog id, which selects its key handling.
    kind: NrDialog,
    destructive: bool,
    title: String,
    field: Option<Entity<Field>>,
    _commands: Option<Subscription>,
    /// Redraws a code's line as its time runs down, and closes the dialog when none is left.
    _ticking: Option<Task<()>>,
    placed: Layout,
    pressed: Option<Part>,
    shade: Move,
    panel: Move,
    done: Done,
}

impl Open {
    fn frame_of(&self, part: Part) -> Option<Bounds<Pixels>> {
        match part {
            Part::Cancel => self.foot().map(|foot| foot.cancel.frame.bounds()),
            Part::Action => self.foot().map(|foot| foot.action.frame.bounds()),
            Part::Close => self.placed.close.map(|close| close.bounds()),
            Part::Outside => None,
        }
    }

    /// Whether `at` is on `part`.
    fn is_on(&self, part: Part, at: Point<Pixels>) -> bool {
        match part {
            Part::Outside => {
                self.placed.code.is_some() && !contains(&self.placed.panel.bounds(), at)
            }
            _ => self
                .frame_of(part)
                .is_some_and(|frame| contains(&frame, at)),
        }
    }

    /// The dialog's two buttons, when it has them.
    fn foot(&self) -> Option<&Foot> {
        self.placed.foot.as_ref()
    }

    fn part_at(&self, at: Point<Pixels>) -> Option<Part> {
        [Part::Cancel, Part::Action, Part::Close, Part::Outside]
            .into_iter()
            .find(|&part| self.is_on(part, at))
    }

    fn typed(&self, cx: &App) -> String {
        self.field
            .as_ref()
            .map_or_else(String::new, |field| field.read(cx).editing.content.clone())
    }
}

/// The window's dialog layer.
pub struct Dialogs {
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    open: Option<Open>,
    /// The open alert box (not on macOS).
    #[cfg(not(target_os = "macos"))]
    alert: Option<alert::OpenAlert>,
}

impl Dialogs {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            previous_focus: None,
            open: None,
            #[cfg(not(target_os = "macos"))]
            alert: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some() || self.alert_open()
    }

    #[cfg(not(target_os = "macos"))]
    fn alert_open(&self) -> bool {
        self.alert.is_some()
    }

    #[cfg(target_os = "macos")]
    fn alert_open(&self) -> bool {
        false
    }

    /// The open dialog's title, for scripted checks.
    pub fn title(&self) -> Option<&str> {
        self.open.as_ref().map(|open| open.title.as_str())
    }

    /// Shows the dialog the core requested (`nr_prompt`). `by_key` is true when a key opened
    /// it; `done` receives the answer.
    pub fn show(&mut self, by_key: bool, done: Done, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = nr_prompt();
        let view = window.viewport_size();
        let placed = place(f32::from(view.width), f32::from(view.height));
        let (field, commands) = match (prompt.has_field, placed.field) {
            (true, Some(place)) => {
                let value = text(&prompt.value);
                let field = cx.new(|cx| {
                    let mut field = Field::new(&text(&prompt.title), &value, window, cx);
                    // The field uses 14 px text. The dialog draws its background and ring so
                    // they scale with the panel.
                    field.face = Face::sans(14.0, 400.0);
                    field.ground = Ground::Bare;
                    field.keys = Keys::Dialog;
                    field.secret = prompt.secret;
                    field.place(place.frame.bounds(), place.control.bounds(), cx);
                    field.editing.select_all();
                    field
                });
                let commands = cx.subscribe_in(&field, window, Self::field_command);
                (Some(field), Some(commands))
            }
            _ => (None, None),
        };
        self.previous_focus = window.focused(cx);
        match &field {
            Some(field) => {
                let handle = field.focus_handle(cx);
                window.focus(&handle, cx);
            }
            None => window.focus(&self.focus, cx),
        }
        nr_dialog_open(prompt.dialog, by_key);
        let ticking = placed.code.is_some().then(|| Self::tick_code(window, cx));
        self.open = Some(Open {
            kind: prompt.dialog,
            destructive: prompt.destructive,
            title: text(&prompt.title),
            field,
            _commands: commands,
            _ticking: ticking,
            placed,
            pressed: None,
            shade: Move::entering(NrMotion::Shade),
            panel: Move::entering(NrMotion::Dialog),
            done,
        });
        cx.notify();
    }

    /// Handles a key the field passes up: the dialog's if the core claims it, otherwise the
    /// field's.
    fn field_command(
        &mut self,
        field: &Entity<Field>,
        command: &Command,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let press = NrKeyPress::named(match command {
            Command::Tab => northroom_core::ffi::NrKey::Tab,
            Command::BackTab => northroom_core::ffi::NrKey::Tab,
            Command::Return => northroom_core::ffi::NrKey::Return,
            Command::Escape => northroom_core::ffi::NrKey::Escape,
        });
        let press = NrKeyPress {
            held: if *command == Command::BackTab {
                northroom_core::ffi::NR_HELD_SHIFT
            } else {
                0
            },
            ..press
        };
        if self.key(press, window, cx) {
            return;
        }
        match command {
            // The field is the only stop in the naming dialog's key loop, so Tab returns to it
            // with its text selected, as in AppKit.
            Command::Tab | Command::BackTab => field.update(cx, |field, cx| {
                field.editing.select_all();
                cx.notify();
            }),
            Command::Return | Command::Escape => {
                let command = *command;
                field.update(cx, |field, cx| field.pass_on(command, window, cx));
            }
        }
    }

    /// Handles a key (`nr_dialog_key`). False when the dialog does not claim it.
    pub fn key(&mut self, press: NrKeyPress, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(open) = &self.open else {
            return false;
        };
        let answer = nr_dialog_key(open.kind, press);
        match answer.intent {
            NrDialogIntent::Focus => cx.notify(),
            NrDialogIntent::Press => self.finish(answer.takes, window, cx),
            NrDialogIntent::Close => self.finish(false, window, cx),
            NrDialogIntent::PassOn => return false,
        }
        true
    }

    /// Handles a key while the dialog itself is focused. A focused field passes up the keys the
    /// dialog decides (`field_command`) and types the rest.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_open() || !self.focus.is_focused(window) {
            return;
        }
        // Keys the dialog ignores still do not reach anything underneath.
        cx.stop_propagation();
        let press = crate::keys::press(&event.keystroke);
        #[cfg(not(target_os = "macos"))]
        if self.alert.is_some() {
            self.alert_key(press, window, cx);
            return;
        }
        self.key(press, window, cx);
    }

    /// What holds focus while the dialog is open, for scripted checks: its field, or itself.
    pub fn keys(&self, window: &Window) -> Option<&'static str> {
        let open = self.open.as_ref()?;
        Some(if self.focus.is_focused(window) || open.field.is_none() {
            "DrawnDialog"
        } else {
            "NSTextView"
        })
    }

    /// Draws a code's line again every `CODE_TICK`, and closes the dialog once the code has no
    /// time left: a phone used it, or it went stale.
    fn tick_code(window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |dialogs, cx| {
            loop {
                cx.background_executor().timer(CODE_TICK).await;
                let going = dialogs.update_in(cx, |dialogs, window, cx| {
                    if nr_pair_left() <= 0.0 {
                        dialogs.finish(false, window, cx);
                        return false;
                    }
                    cx.notify();
                    true
                });
                if !matches!(going, Ok(true)) {
                    break;
                }
            }
        })
    }

    /// Answers the dialog: action taken (`taken`) or cancelled. It stays open when a required
    /// name is missing.
    fn finish(&mut self, taken: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &self.open else {
            return;
        };
        let typed = c(&open.typed(cx));
        // SAFETY: a NUL-terminated string.
        let answer = unsafe { nr_prompt_answer(taken, typed.as_ptr()) };
        if answer.effect == NrEffect::Stays {
            return;
        }
        let open = self.open.take().expect("a dialog is open");
        nr_dialog_close(open.kind);
        match self.previous_focus.take() {
            Some(previous) => window.focus(&previous, cx),
            None => window.blur(cx),
        }
        cx.notify();
        let done = open.done.clone();
        window.defer(cx, move |window, cx| done(answer, window, cx));
    }

    // MARK: Pointer

    fn mouse_down(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.open {
            open.pressed = open.part_at(at);
            cx.notify();
        }
    }

    fn mouse_up(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = &mut self.open else {
            return;
        };
        let was = open.pressed.take();
        cx.notify();
        let Some(part) = was else {
            return;
        };
        if !open.is_on(part, at) {
            return;
        }
        self.finish(part == Part::Action, window, cx);
    }

    // MARK: Drawing

    fn paint(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = window.viewport_size();
        let Some(open) = &mut self.open else {
            return;
        };
        open.placed = place(f32::from(view.width), f32::from(view.height));
        let whole = Bounds {
            origin: Point::default(),
            size: view,
        };
        let shade = open.shade.frame();
        window.paint_quad(gpui::fill(whole, veil(0.5 * shade.opacity as f32)));
        let panel = open.placed.panel.bounds();
        // The panel scales about its centre, in window points.
        let pose = Pose::of(
            &open.panel.frame(),
            Point::default(),
            Point::default(),
            panel.center(),
        );
        // The panel and its contents appear as one group.
        let painter = DialogPainter {
            open,
            pose: Pose {
                opacity: 1.0,
                ..pose
            },
        };
        crate::layer::group(window, cx, pose.opacity, |window, cx| {
            painter.draw(window, cx);
        });
        if open.shade.runs() || open.panel.runs() {
            window.request_animation_frame();
        }
        Self::listen_to_pointer(window, cx);
    }

    /// Captures pointer events for this frame so nothing under the dialog receives them.
    fn listen_to_pointer(window: &mut Window, cx: &mut Context<Self>) {
        let dialogs = cx.entity();
        window.on_mouse_event({
            let dialogs = dialogs.clone();
            move |event: &MouseDownEvent, phase, _, cx| {
                // Block everything underneath; the field handles its own presses.
                if phase == DispatchPhase::Bubble {
                    cx.stop_propagation();
                    if event.button == MouseButton::Left {
                        dialogs.update(cx, |dialogs, cx| dialogs.mouse_down(event.position, cx));
                    }
                }
            }
        });
        window.on_mouse_event({
            let dialogs = dialogs.clone();
            move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    cx.stop_propagation();
                    dialogs.update(cx, |dialogs, cx| {
                        dialogs.mouse_up(event.position, window, cx)
                    });
                }
            }
        });
        window.on_mouse_event(move |_: &ScrollEvent, phase, _, cx| {
            if phase == DispatchPhase::Bubble {
                cx.stop_propagation();
            }
        });
    }
}

/// Draws the open dialog with its panel pose at full opacity.
struct DialogPainter<'a> {
    open: &'a Open,
    pose: Pose,
}

impl DialogPainter<'_> {
    /// Draws the panel and its title, close mark, message, field, Cancel and action.
    fn draw(&self, window: &mut Window, cx: &mut App) {
        let (pose, placed) = (&self.pose, &self.open.placed);
        if let Some(code) = &placed.code {
            self.draw_code(window, code);
            return;
        }
        window.paint_quad(gpui::quad(
            pose.rect(placed.panel.bounds()),
            px(pose.length(14.0)),
            pose.color(color(Token::Background)),
            px(pose.length(1.0)),
            pose.color(color(Token::Border)),
            gpui::BorderStyle::Solid,
        ));
        if let Some(title) = &placed.title {
            self.line(window, cx, title, None);
        }
        if let Some(close) = placed.close {
            self.draw_close(window, cx, close.bounds());
        }
        for sentence in &placed.message {
            self.line(window, cx, sentence, None);
        }
        if let Some(field) = placed.field {
            self.draw_field(window, field.frame.bounds());
        }
        if let Some(foot) = self.open.foot() {
            self.draw_cancel(window, cx, &foot.cancel);
            self.draw_focus_ring(window, foot);
            self.draw_action(window, cx, &foot.action);
        }
    }

    /// Draws a QR code dark on light whatever the theme, as a camera reads it: the card, a dot
    /// for each dark square, the corner marks, and the line around it for the time left.
    fn draw_code(&self, window: &mut Window, code: &CodePlace) {
        let (light, dark) = (gpui::white(), gpui::black());
        self.rounded(
            window,
            code.ground.bounds(),
            light,
            code.ground_radius as f32,
        );
        for dot in &code.dots {
            self.rounded(window, dot.bounds(), dark, (dot.width / 2.0) as f32);
        }
        for mark in &code.marks {
            for (square, ink) in [(mark.dark, dark), (mark.light, light), (mark.middle, dark)] {
                let radius = (square.width * CornerMark::ROUND) as f32;
                self.rounded(window, square.bounds(), ink, radius);
            }
        }
        self.draw_code_line(window, code, &code.line_left(1.0), light.alpha(0.16));
        self.draw_code_line(window, code, &code.line_left(nr_pair_left()), light);
    }

    /// Strokes `points` of the line around a code in `ink`.
    fn draw_code_line(
        &self,
        window: &mut Window,
        code: &CodePlace,
        points: &[(f64, f64)],
        ink: Hsla,
    ) {
        let Some((first, rest)) = points.split_first() else {
            return;
        };
        let pose = &self.pose;
        let place = |at: &(f64, f64)| pose.place(at.0 as f32, at.1 as f32);
        let mut path = PathBuilder::stroke(px(pose.length(code.line_width as f32)));
        path.move_to(place(first));
        for point in rest {
            path.line_to(place(point));
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, pose.color(ink));
        }
    }

    /// Draws a layout line in its own colour, or in `ink` if given.
    fn line(&self, window: &mut Window, cx: &mut App, line: &Line, ink: Option<Hsla>) {
        self.pose.line(
            window,
            cx,
            &line.text,
            line.font.face(),
            ink.unwrap_or_else(|| line.ink()),
            line.frame.bounds(),
            line.frame.width as f32,
        );
    }

    fn rounded(&self, window: &mut Window, frame: Bounds<Pixels>, ink: Hsla, radius: f32) {
        let pose = &self.pose;
        window.paint_quad(
            gpui::fill(pose.rect(frame), pose.color(ink)).corner_radii(px(pose.length(radius))),
        );
    }

    fn stroke(
        &self,
        window: &mut Window,
        frame: Bounds<Pixels>,
        ink: Hsla,
        radius: f32,
        width: f32,
    ) {
        let pose = &self.pose;
        crate::draw::stroke(
            window,
            pose.rect(frame),
            pose.color(ink),
            pose.length(radius),
            pose.length(width),
        );
    }

    /// Draws the close mark, muted until pressed.
    fn draw_close(&self, window: &mut Window, cx: &mut App, close: Bounds<Pixels>) {
        let strength = if self.open.pressed == Some(Part::Close) {
            1.0
        } else {
            0.7
        };
        let ink = color(Token::Foreground);
        icons::draw(
            window,
            cx,
            Glyph::Close,
            self.pose.rect(close),
            self.pose.color(ink.alpha(ink.a * strength)),
        );
    }

    /// Draws the field's background and focus ring: a 1 px line plus 2 px at 30% around it.
    fn draw_field(&self, window: &mut Window, frame: Bounds<Pixels>) {
        self.rounded(window, frame, color(Token::Field), 6.0);
        self.stroke(
            window,
            inset(frame, -1.0, -1.0),
            mixed(Token::Primary, 0.3),
            7.0,
            2.0,
        );
        self.stroke(
            window,
            inset(frame, 0.5, 0.5),
            color(Token::Primary),
            5.5,
            1.0,
        );
    }

    /// Draws Cancel: an outline, filled while pressed.
    fn draw_cancel(&self, window: &mut Window, cx: &mut App, cancel: &Button) {
        let frame = cancel.frame.bounds();
        if self.open.pressed == Some(Part::Cancel) {
            self.rounded(window, frame, mixed(Token::Foreground, 0.12), 6.0);
        }
        self.stroke(
            window,
            inset(frame, 0.5, 0.5),
            color(Token::Border),
            5.5,
            1.0,
        );
        self.line(window, cx, &cancel.words, None);
    }

    /// Draws the focused button's ring when no field has focus: 2 px in the ring colour, 1 px
    /// outside.
    fn draw_focus_ring(&self, window: &mut Window, foot: &Foot) {
        let open = self.open;
        let focus = nr_dialog_focus(open.kind);
        if open.field.is_some() || !focus.ring || focus.part < 0 {
            return;
        }
        let around = if focus.part == 0 {
            &foot.cancel
        } else {
            &foot.action
        };
        self.stroke(
            window,
            inset(around.frame.bounds(), -2.0, -2.0),
            color(Token::Ring),
            8.0,
            2.0,
        );
    }

    /// Draws the action button in the accent, or the warning colour when destructive. It is
    /// faded while the typed text is not accepted and darker while pressed.
    fn draw_action(&self, window: &mut Window, cx: &mut App, action: &Button) {
        let open = self.open;
        let typed = c(&open.typed(cx));
        // SAFETY: a NUL-terminated string.
        let enabled = unsafe { nr_prompt_takes(typed.as_ptr()) };
        let strength = if enabled { 1.0 } else { 0.5 };
        let fill = color(if open.destructive {
            Token::Destructive
        } else {
            Token::Primary
        });
        let pressed = if open.pressed == Some(Part::Action) {
            0.9
        } else {
            1.0
        };
        self.rounded(
            window,
            action.frame.bounds(),
            fill.alpha(pressed * strength),
            6.0,
        );
        let words = &action.words;
        self.line(window, cx, words, Some(color(words.color).alpha(strength)));
    }
}

/// The scrim colour: ink at `share` opacity, matching an 8-bit compositor. Core Graphics blends
/// in bytes and truncates; GPUI blends in floats and rounds, which leaves the scrim half a step
/// lighter (dark theme 21 becomes 13 where Core Graphics gives 12). Lowering the ink by half a
/// step divided by `share` makes GPUI's rounding land on Core Graphics' result.
pub(crate) fn veil(share: f32) -> Hsla {
    let ink: Rgba = color(Token::Ink).into();
    if share <= 0.0 {
        return ink.into();
    }
    Rgba {
        r: cut(ink.r, share),
        g: cut(ink.g, share),
        b: cut(ink.b, share),
        a: share,
    }
    .into()
}

/// Lowers a channel blended at `share` by half a step, so GPUI's rounding matches an 8-bit
/// compositor's truncation.
fn cut(channel: f32, share: f32) -> f32 {
    (channel - 0.5 / 255.0 / share).max(0.0)
}

impl Render for Dialogs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.is_open() {
            return div().into_any_element();
        }
        let field = self.open.as_ref().and_then(|open| open.field.clone());
        let view = window.viewport_size();
        let this = cx.entity();
        let layer = div()
            .id("dialog")
            .absolute()
            .top_0()
            .left_0()
            .w(view.width)
            .h(view.height)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .child(
                canvas(
                    move |bounds, window, _| {
                        window.insert_hitbox(bounds, HitboxBehavior::BlockMouse);
                    },
                    move |_, (), window, cx| {
                        this.update(cx, |dialogs, cx| {
                            #[cfg(not(target_os = "macos"))]
                            if dialogs.alert.is_some() {
                                dialogs.paint_alert(window, cx);
                                return;
                            }
                            dialogs.paint(window, cx)
                        })
                    },
                )
                .size_full(),
            )
            .children(field);
        crate::popover::layer(layer, crate::popover::Layer::Dialog)
    }
}

#[cfg(test)]
mod tests {
    use super::cut;

    /// Core Graphics' result: byte blend of `under` and `over` at a byte alpha, truncated.
    fn core_graphics(under: f32, over: f32, share: f32) -> f32 {
        let alpha = (share * 255.0).round();
        ((under * (256.0 - alpha) + over * alpha) / 256.0).floor()
    }

    /// GPUI's result: float blend rounded to a byte.
    fn gpui(under: f32, over: f32, share: f32) -> f32 {
        (under * (1.0 - share) + over * share).round()
    }

    #[test]
    fn the_veil_lands_where_core_graphics_cuts_it_over_every_ground() {
        // The dark theme's ink over its window backgrounds, at 50%.
        let ink = 4.0;
        for under in [17.0, 21.0, 25.0, 28.0, 49.0, 238.0] {
            let cut_ink = cut(ink / 255.0, 0.5) * 255.0;
            assert_eq!(
                gpui(under, cut_ink, 0.5),
                core_graphics(under, ink, 0.5),
                "over {under}"
            );
        }
    }
}
